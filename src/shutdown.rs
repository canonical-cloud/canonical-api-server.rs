use std::{
    io::{self, IsTerminal, Read},
    net::SocketAddr,
    time::{Duration, Instant},
};

use axum::Router;
use axum_server::Handle;
use next_loggers::{
    HttpShutdownController, ShutdownDecision, ShutdownPhase, ShutdownTrigger,
    DEFAULT_HTTP_GRACE_PERIOD,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::sleep};

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

    const fn shared_trigger(self) -> Option<ShutdownTrigger> {
        match self {
            Self::SigInt => Some(ShutdownTrigger::SigInt),
            Self::SigTerm => Some(ShutdownTrigger::SigTerm),
            Self::Eof => Some(ShutdownTrigger::StdinEof),
            Self::Deadline => Some(ShutdownTrigger::Deadline),
            Self::DrainFailed => None,
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
            grace: DEFAULT_HTTP_GRACE_PERIOD,
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

pub async fn serve(listener: TcpListener, router: Router, config: Config) -> io::Result<Outcome> {
    let (events_tx, events_rx) = event_channel();
    serve_with_events(listener, router, config, events_tx, events_rx).await
}

async fn serve_with_events(
    listener: TcpListener,
    router: Router,
    config: Config,
    events_tx: mpsc::UnboundedSender<Event>,
    mut events_rx: mpsc::UnboundedReceiver<Event>,
) -> io::Result<Outcome> {
    let listener = listener.into_std()?;
    listener.set_nonblocking(true)?;

    let started_at = Instant::now();
    let handle: Handle<SocketAddr> = Handle::new();
    let lifecycle = HttpShutdownController::new(config.grace);
    let mut trigger: Option<Event> = None;
    let mut forced_by: Option<Event> = None;
    let mut signal_count = 0_u32;
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
                        if lifecycle.phase() == ShutdownPhase::Draining {
                            let _ = lifecycle.handle_trigger(
                                ShutdownTrigger::GracefulComplete,
                                false,
                            );
                        }
                        let outcome = match forced_by {
                            Some(event) => Outcome::Forced(event),
                            None => Outcome::Graceful,
                        };
                        tracing::info!(
                            event = "server.shutdown.complete",
                            outcome = ?outcome,
                            phase = ?lifecycle.phase(),
                            trigger = ?trigger,
                            forced_by = ?forced_by,
                            tty = config.tty,
                            signal_count,
                            grace_ms = millis(config.grace),
                            active_connections = handle.connection_count() as u64,
                            elapsed_ms = millis(started_at.elapsed()),
                            "server shutdown complete",
                        );
                        return Ok(outcome);
                    }
                    Err(error) => {
                        lifecycle.gate().stop_accepting();
                        forced_by.get_or_insert(Event::DrainFailed);
                        tracing::error!(
                            event = "server.shutdown.complete",
                            outcome = "serve-failed",
                            phase = ?lifecycle.phase(),
                            trigger = ?trigger,
                            forced_by = ?forced_by,
                            tty = config.tty,
                            signal_count,
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
                        events_open = false;
                        Event::DrainFailed
                    }
                };

                if event.is_signal() {
                    signal_count = signal_count.saturating_add(1);
                }
                let active_connections = handle.connection_count() as u64;

                if event == Event::DrainFailed {
                    lifecycle.gate().stop_accepting();
                    forced_by = Some(event);
                    if let Some(task) = deadline_task.take() {
                        task.abort();
                    }
                    tracing::warn!(
                        event = "server.shutdown",
                        input = ?event,
                        phase = ?lifecycle.phase(),
                        trigger = ?trigger,
                        forced_by = ?forced_by,
                        tty = config.tty,
                        signal_count,
                        grace_ms = millis(config.grace),
                        active_connections,
                        elapsed_ms = millis(started_at.elapsed()),
                        "forcing shutdown because the lifecycle event channel failed",
                    );
                    handle.shutdown();
                    continue;
                }

                let Some(shared_trigger) = event.shared_trigger() else {
                    continue;
                };
                let signal_outcome = lifecycle.handle_trigger(shared_trigger, config.tty);

                match signal_outcome.decision {
                    ShutdownDecision::Ignore => {}
                    ShutdownDecision::Drain => {
                        trigger.get_or_insert(event);
                        tracing::info!(
                            event = "server.shutdown",
                            input = ?event,
                            phase = ?lifecycle.phase(),
                            trigger = ?trigger,
                            tty = config.tty,
                            signal_count,
                            grace_ms = millis(config.grace),
                            active_connections,
                            elapsed_ms = millis(started_at.elapsed()),
                            "shutdown requested; listener is closing and active work is draining",
                        );

                        if let Some(warning) = signal_outcome.terminal_warning.as_deref() {
                            if config.watch_stdin_eof && !eof_watcher_armed {
                                eof_watcher_armed = true;
                                spawn_stdin_eof_watcher(events_tx.clone());
                            }
                            tracing::info!(
                                event = "server.shutdown",
                                input = ?event,
                                phase = ?lifecycle.phase(),
                                tty = config.tty,
                                signal_count,
                                warning,
                                "interactive drain active",
                            );
                        }

                        handle.graceful_shutdown(None);
                        let deadline_tx = events_tx.clone();
                        let grace = config.grace;
                        deadline_task = Some(tokio::spawn(async move {
                            sleep(grace).await;
                            let _ = deadline_tx.send(Event::Deadline);
                        }));
                    }
                    ShutdownDecision::Force => {
                        forced_by = Some(event);
                        if let Some(task) = deadline_task.take() {
                            task.abort();
                        }
                        tracing::warn!(
                            event = "server.shutdown",
                            input = ?event,
                            phase = ?lifecycle.phase(),
                            trigger = ?trigger,
                            forced_by = ?forced_by,
                            tty = config.tty,
                            signal_count,
                            grace_ms = millis(config.grace),
                            active_connections,
                            elapsed_ms = millis(started_at.elapsed()),
                            "forcing shutdown; active HTTP and WebSocket connections will be dropped",
                        );
                        handle.shutdown();
                    }
                    ShutdownDecision::Complete => {}
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
        time::{sleep, timeout},
    };

    use super::*;

    #[test]
    fn tty_second_sigint_waits_for_ctrl_d() {
        let lifecycle = HttpShutdownController::default();
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigInt, true)
                .decision,
            ShutdownDecision::Drain
        );
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigInt, true)
                .decision,
            ShutdownDecision::Ignore
        );
        assert_eq!(lifecycle.phase(), ShutdownPhase::Draining);
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::StdinEof, true)
                .decision,
            ShutdownDecision::Force
        );
    }

    #[test]
    fn second_sigterm_forces() {
        let lifecycle = HttpShutdownController::default();
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigTerm, false)
                .decision,
            ShutdownDecision::Drain
        );
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigTerm, false)
                .decision,
            ShutdownDecision::Force
        );
    }

    #[test]
    fn tty_eof_forces_any_active_drain() {
        let lifecycle = HttpShutdownController::default();
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigTerm, true)
                .decision,
            ShutdownDecision::Drain
        );
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::StdinEof, true)
                .decision,
            ShutdownDecision::Force
        );
    }

    #[test]
    fn non_tty_eof_is_ignored_and_one_sigterm_drains() {
        let lifecycle = HttpShutdownController::default();
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::StdinEof, false)
                .decision,
            ShutdownDecision::Ignore
        );
        assert_eq!(
            lifecycle
                .handle_trigger(ShutdownTrigger::SigTerm, false)
                .decision,
            ShutdownDecision::Drain
        );
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
    async fn repeated_tty_sigint_waits_until_ctrl_d() {
        let (address, entered, events, task) = active_server(Duration::from_secs(2)).await;
        let mut stream = open_slow_request(address, entered).await;

        events.send(Event::SigInt).unwrap();
        tokio::task::yield_now().await;
        events.send(Event::SigInt).unwrap();
        sleep(Duration::from_millis(25)).await;
        assert!(!task.is_finished(), "second interactive SIGINT forced the server");

        events.send(Event::Eof).unwrap();
        let outcome = timeout(Duration::from_secs(1), task)
            .await
            .expect("server did not force close after EOF")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, Outcome::Forced(Event::Eof));

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
