use std::{
    io::{self, IsTerminal, Read},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use axum::Router;
use axum_server::Handle;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::sleep};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Running,
    Draining,
    Forcing,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    SigInt,
    SigTerm,
    Eof,
    Deadline,
    DrainFailed,
}

impl Event {
    const fn is_signal(self) -> bool {
        matches!(self, Self::SigInt | Self::SigTerm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    StartGraceful,
    Force,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct State {
    phase: Phase,
    tty: bool,
    trigger: Option<Event>,
    /// Counts operating-system SIGINT/SIGTERM events only.
    signal_count: u32,
    forced_by: Option<Event>,
}

impl State {
    const fn new(tty: bool) -> Self {
        Self {
            phase: Phase::Running,
            tty,
            trigger: None,
            signal_count: 0,
            forced_by: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub grace: Duration,
    pub tty: bool,
    pub watch_stdin_eof: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(30),
            tty: std::io::stdin().is_terminal(),
            watch_stdin_eof: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Graceful,
    Forced(Event),
}

const fn reduce(state: State, event: Event) -> (State, Action) {
    match state.phase {
        Phase::Stopped | Phase::Forcing => (state, Action::None),
        Phase::Running => {
            if matches!(event, Event::Eof) {
                // Ctrl-D is armed only after the first interactive SIGINT.
                // Before that point stdin belongs to the application.
                return (state, Action::None);
            }

            if event.is_signal() {
                let mut next = state;
                next.phase = Phase::Draining;
                next.trigger = Some(event);
                next.signal_count = next.signal_count.saturating_add(1);
                (next, Action::StartGraceful)
            } else if matches!(event, Event::DrainFailed) {
                let mut next = state;
                next.phase = Phase::Forcing;
                next.trigger = Some(event);
                next.forced_by = Some(event);
                (next, Action::Force)
            } else {
                (state, Action::None)
            }
        }
        Phase::Draining => {
            let force = matches!(
                event,
                Event::Deadline | Event::DrainFailed | Event::SigInt | Event::SigTerm
            ) || (matches!(event, Event::Eof)
                && state.tty
                && matches!(state.trigger, Some(Event::SigInt)));

            if !force {
                return (state, Action::None);
            }

            let mut next = state;
            next.phase = Phase::Forcing;
            next.forced_by = Some(event);
            if event.is_signal() {
                next.signal_count = next.signal_count.saturating_add(1);
            }
            (next, Action::Force)
        }
    }
}

fn event_channel() -> (mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded_channel();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let sigint_tx = tx.clone();
        tokio::spawn(async move {
            match signal(SignalKind::interrupt()) {
                Ok(mut stream) => {
                    while stream.recv().await.is_some() {
                        if sigint_tx.send(Event::SigInt).is_err() {
                            break;
                        }
                    }
                }
                Err(error) => tracing::error!(%error, "failed to install SIGINT handler"),
            }
        });

        let sigterm_tx = tx.clone();
        tokio::spawn(async move {
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    while stream.recv().await.is_some() {
                        if sigterm_tx.send(Event::SigTerm).is_err() {
                            break;
                        }
                    }
                }
                Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
            }
        });
    }

    #[cfg(not(unix))]
    {
        let sigint_tx = tx.clone();
        tokio::spawn(async move {
            loop {
                match tokio::signal::ctrl_c().await {
                    Ok(()) => {
                        if sigint_tx.send(Event::SigInt).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "failed to wait for Ctrl-C");
                        break;
                    }
                }
            }
        });
    }

    (tx, rx)
}

fn spawn_stdin_eof_watcher(eof_tx: mpsc::UnboundedSender<Event>) {
    let spawn_result = std::thread::Builder::new()
        .name("shutdown-stdin-eof".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut stdin = stdin.lock();
            let mut buffer = [0_u8; 256];

            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => {
                        let _ = eof_tx.send(Event::Eof);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "stdin EOF watcher stopped");
                        break;
                    }
                }
            }
        });

    if let Err(error) = spawn_result {
        tracing::error!(%error, "failed to start stdin EOF watcher");
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub async fn serve(
    listener: TcpListener,
    router: Router,
    config: Config,
    accepting: Arc<AtomicBool>,
) -> io::Result<Outcome> {
    let (events_tx, events_rx) = event_channel();
    serve_with_events(
        listener,
        router,
        config,
        Some(accepting),
        events_tx,
        events_rx,
    )
    .await
}

async fn serve_with_events(
    listener: TcpListener,
    router: Router,
    config: Config,
    accepting: Option<Arc<AtomicBool>>,
    events_tx: mpsc::UnboundedSender<Event>,
    mut events_rx: mpsc::UnboundedReceiver<Event>,
) -> io::Result<Outcome> {
    let listener = listener.into_std()?;
    listener.set_nonblocking(true)?;

    let started_at = Instant::now();
    let handle: Handle<SocketAddr> = Handle::new();
    let mut state = State::new(config.tty);
    let mut deadline_task: Option<JoinHandle<()>> = None;
    let mut eof_watcher_armed = false;
    let mut events_open = true;
    let server = axum_server::from_tcp(listener)?;
    let mut server = Box::pin(
        server
            .handle(handle.clone())
            .serve(router.into_make_service()),
    );

    loop {
        tokio::select! {
            result = &mut server => {
                if let Some(task) = deadline_task.take() {
                    task.abort();
                }

                match result {
                    Ok(()) => {
                        state.phase = Phase::Stopped;
                        let outcome = match state.forced_by {
                            Some(event) => Outcome::Forced(event),
                            None => Outcome::Graceful,
                        };
                        tracing::info!(
                            event = "server.shutdown.complete",
                            outcome = ?outcome,
                            phase = ?state.phase,
                            trigger = ?state.trigger,
                            forced_by = ?state.forced_by,
                            tty = state.tty,
                            signal_count = state.signal_count,
                            grace_ms = millis(config.grace),
                            active_connections = handle.connection_count() as u64,
                            elapsed_ms = millis(started_at.elapsed()),
                            "server shutdown complete",
                        );
                        return Ok(outcome);
                    }
                    Err(error) => {
                        let (next, action) = reduce(state, Event::DrainFailed);
                        state = next;
                        if matches!(action, Action::Force) {
                            handle.shutdown();
                        }
                        tracing::error!(
                            event = "server.shutdown.complete",
                            outcome = "serve-failed",
                            phase = ?state.phase,
                            trigger = ?state.trigger,
                            forced_by = ?state.forced_by,
                            tty = state.tty,
                            signal_count = state.signal_count,
                            grace_ms = millis(config.grace),
                            active_connections = handle.connection_count() as u64,
                            elapsed_ms = millis(started_at.elapsed()),
                            %error,
                            "server loop failed",
                        );
                        return Err(error);
                    }
                }
            }
            event = events_rx.recv(), if events_open => {
                let event = match event {
                    Some(event) => event,
                    None => {
                        // Losing every lifecycle input must fail closed rather
                        // than leaving a server that can no longer be stopped.
                        events_open = false;
                        Event::DrainFailed
                    }
                };

                let (next, action) = reduce(state, event);
                state = next;
                let active_connections = handle.connection_count() as u64;

                match action {
                    Action::None => {}
                    Action::StartGraceful => {
                        if let Some(accepting) = &accepting {
                            accepting.store(false, Ordering::Release);
                        }
                        tracing::info!(
                            event = "server.shutdown",
                            input = ?event,
                            phase = ?state.phase,
                            trigger = ?state.trigger,
                            tty = state.tty,
                            signal_count = state.signal_count,
                            grace_ms = millis(config.grace),
                            active_connections,
                            elapsed_ms = millis(started_at.elapsed()),
                            "shutdown requested; listener is closing and active work is draining",
                        );

                        if state.tty && matches!(event, Event::SigInt) {
                            if config.watch_stdin_eof && !eof_watcher_armed {
                                eof_watcher_armed = true;
                                spawn_stdin_eof_watcher(events_tx.clone());
                            }
                            tracing::info!(
                                event = "server.shutdown",
                                input = ?event,
                                phase = ?state.phase,
                                tty = true,
                                signal_count = state.signal_count,
                                "interactive drain active; press Ctrl-C again or Ctrl-D to force close",
                            );
                        }

                        // The reducer owns the deadline so a timeout is logged
                        // as forced rather than silently reported as graceful.
                        handle.graceful_shutdown(None);
                        let deadline_tx = events_tx.clone();
                        let grace = config.grace;
                        deadline_task = Some(tokio::spawn(async move {
                            sleep(grace).await;
                            let _ = deadline_tx.send(Event::Deadline);
                        }));
                    }
                    Action::Force => {
                        if let Some(task) = deadline_task.take() {
                            task.abort();
                        }
                        tracing::warn!(
                            event = "server.shutdown",
                            input = ?event,
                            phase = ?state.phase,
                            trigger = ?state.trigger,
                            forced_by = ?state.forced_by,
                            tty = state.tty,
                            signal_count = state.signal_count,
                            grace_ms = millis(config.grace),
                            active_connections,
                            elapsed_ms = millis(started_at.elapsed()),
                            "forcing shutdown; active HTTP and WebSocket connections will be dropped",
                        );
                        handle.shutdown();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{future, sync::Arc};

    use axum::{extract::State as AxumState, routing::get};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::{mpsc, Notify},
        time::timeout,
    };

    use super::*;

    #[test]
    fn tty_second_sigint_forces() {
        let (state, action) = reduce(State::new(true), Event::SigInt);
        assert_eq!(action, Action::StartGraceful);
        assert_eq!(state.signal_count, 1);

        let (state, action) = reduce(state, Event::SigInt);
        assert_eq!(action, Action::Force);
        assert_eq!(state.phase, Phase::Forcing);
        assert_eq!(state.signal_count, 2);
    }

    #[test]
    fn sigterm_counts_as_an_operating_system_signal() {
        let (state, action) = reduce(State::new(false), Event::SigTerm);
        assert_eq!(action, Action::StartGraceful);
        assert_eq!(state.signal_count, 1);

        let (state, action) = reduce(state, Event::SigTerm);
        assert_eq!(action, Action::Force);
        assert_eq!(state.signal_count, 2);
    }

    #[test]
    fn tty_eof_only_forces_after_first_sigint_without_counting_a_signal() {
        let initial = State::new(true);
        assert_eq!(reduce(initial, Event::Eof), (initial, Action::None));

        let (state, _) = reduce(initial, Event::SigInt);
        let (state, action) = reduce(state, Event::Eof);
        assert_eq!(action, Action::Force);
        assert_eq!(state.forced_by, Some(Event::Eof));
        assert_eq!(state.signal_count, 1);
    }

    #[test]
    fn tty_eof_after_sigterm_is_ignored() {
        let (state, _) = reduce(State::new(true), Event::SigTerm);
        assert_eq!(state.signal_count, 1);
        assert_eq!(reduce(state, Event::Eof), (state, Action::None));
    }

    #[test]
    fn non_tty_eof_is_ignored_and_one_sigterm_drains() {
        let initial = State::new(false);
        assert_eq!(reduce(initial, Event::Eof), (initial, Action::None));
        let (state, action) = reduce(initial, Event::SigTerm);
        assert_eq!(action, Action::StartGraceful);
        assert_eq!(state.phase, Phase::Draining);
        assert_eq!(state.signal_count, 1);
    }

    #[test]
    fn deadline_and_drain_failure_do_not_increment_signal_count() {
        for event in [Event::Deadline, Event::DrainFailed] {
            let (state, _) = reduce(State::new(false), Event::SigTerm);
            let (forced, action) = reduce(state, event);
            assert_eq!(action, Action::Force);
            assert_eq!(forced.signal_count, 1);
        }
    }

    async fn never_finishes(AxumState(entered): AxumState<Arc<Notify>>) -> &'static str {
        entered.notify_one();
        future::pending::<()>().await;
        "unreachable"
    }

    async fn active_server(
        grace: Duration,
    ) -> (
        SocketAddr,
        Arc<Notify>,
        mpsc::UnboundedSender<Event>,
        tokio::task::JoinHandle<io::Result<Outcome>>,
    ) {
        let entered = Arc::new(Notify::new());
        let app = Router::new()
            .route("/slow", get(never_finishes))
            .with_state(entered.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let server_tx = events_tx.clone();
        let task = tokio::spawn(serve_with_events(
            listener,
            app,
            Config {
                grace,
                tty: true,
                watch_stdin_eof: false,
            },
            None,
            server_tx,
            events_rx,
        ));
        (address, entered, events_tx, task)
    }

    async fn open_slow_request(address: SocketAddr, entered: Arc<Notify>) -> TcpStream {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .unwrap();
        timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("handler did not start");
        stream
    }

    #[tokio::test]
    async fn second_tty_sigint_force_closes_active_connection() {
        let (address, entered, events, task) = active_server(Duration::from_secs(2)).await;
        let mut stream = open_slow_request(address, entered).await;

        events.send(Event::SigInt).unwrap();
        tokio::task::yield_now().await;
        events.send(Event::SigInt).unwrap();

        let outcome = timeout(Duration::from_secs(1), task)
            .await
            .expect("server did not force close")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, Outcome::Forced(Event::SigInt));

        let mut byte = [0_u8; 1];
        let read = timeout(Duration::from_secs(1), stream.read(&mut byte))
            .await
            .expect("connection remained open");
        assert!(matches!(read, Ok(0) | Err(_)));
    }

    #[tokio::test]
    async fn deadline_force_closes_active_connection() {
        let (address, entered, events, task) = active_server(Duration::from_millis(20)).await;
        let _stream = open_slow_request(address, entered).await;

        events.send(Event::SigTerm).unwrap();
        let outcome = timeout(Duration::from_secs(1), task)
            .await
            .expect("deadline did not force close")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, Outcome::Forced(Event::Deadline));
    }

    #[tokio::test]
    async fn one_non_tty_sigterm_is_graceful_when_idle() {
        let app = Router::new().route("/", get(|| async { "ok" }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let server_tx = events_tx.clone();
        let task = tokio::spawn(serve_with_events(
            listener,
            app,
            Config {
                grace: Duration::from_secs(1),
                tty: false,
                watch_stdin_eof: false,
            },
            None,
            server_tx,
            events_rx,
        ));

        events_tx.send(Event::SigTerm).unwrap();
        let outcome = timeout(Duration::from_secs(1), task)
            .await
            .expect("server did not stop")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, Outcome::Graceful);
    }
}
