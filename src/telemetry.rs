//! Ores structured logging bridged into the service's JSON tracing stream.
//!
//! The bridge installs one subscriber and never attaches credentials, URLs,
//! request bodies, identity values, or upstream response bodies.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, PoisonError, RwLock,
};

use next_loggers::{
    json, JsonObject, LogLevel, LogRecord, Logger, LoggerError, Options, Transport,
};
use tracing_subscriber::EnvFilter;

const SERVICE_NAME: &str = "canonical-api-server";
const SERVICE_NAMESPACE: &str = "canonical-cloud";

/// The application logger for the ACTIVE telemetry lifetime.
///
/// RPC error logging runs on an error path that can be hot, and building a
/// Logger there allocated a fresh transport per failed call. The logger is
/// built once per lifetime and shared.
///
/// Restartable on purpose. This was a `OnceLock<Logger>` whose guard closed it
/// on drop: after the first guard dropped, the lock could only ever hand back
/// that same closed logger — `get_or_init` never rebuilds — so a late RPC error,
/// or any test that ran telemetry twice, emitted into a closed logger and
/// nothing said so. Ownership is now explicit: the slot owns the logger, the
/// guard RETIRES it (takes it out, then closes it), and the next emitter finds
/// the slot empty and builds a fresh one.
static ORES_LOGGER: RwLock<Option<Arc<Logger>>> = RwLock::new(None);

/// How many loggers have been built in this process. One per active lifetime
/// is the invariant the hot path depends on; the tests assert it.
static LOGGER_BUILDS: AtomicUsize = AtomicUsize::new(0);

fn build_logger() -> Logger {
    LOGGER_BUILDS.fetch_add(1, Ordering::Relaxed);
    Logger::new(Options {
        app_name: SERVICE_NAME.to_string(),
        name: Some("server".to_string()),
        console: false,
        transports: vec![Arc::new(TracingBridgeTransport)],
        ..Options::default()
    })
}

/// Run `emit` with the active logger, holding the slot's READ lock for the
/// whole emission.
///
/// The lock has to cover the write, not just the lookup. An earlier version
/// handed out an `Arc<Logger>` and released the lock: an emitter could clone
/// the Arc, lose the CPU, and have retirement take and close the logger before
/// it wrote — and a closed next-loggers logger panics. That is the very failure
/// retiring was introduced to remove, moved from "after shutdown" to "during
/// it". With the read guard held here, retire_logger() cannot take its write
/// lock until every emission in flight has finished, and no emission can start
/// on a logger that is about to be closed.
///
/// `emit` must not log through this function again: std's RwLock may park a
/// second read behind a waiting writer, and the two would deadlock. The only
/// transport is the tracing bridge, which does not.
///
/// A poisoned lock is recovered rather than propagated: this runs on error
/// paths, and a panic elsewhere must not turn logging into a second panic.
fn with_ores_logger<T>(emit: impl FnOnce(&Logger) -> T) -> T {
    let mut emit = Some(emit);
    loop {
        {
            let slot = ORES_LOGGER.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(logger) = slot.as_ref() {
                let emit = emit.take().expect("emit runs exactly once");
                return emit(logger);
            }
        }
        // Empty: first use, or retired. Build under the write lock, then go
        // round again to take the read lock — std cannot downgrade a write
        // guard, and a retirement may land in between, hence the loop.
        ORES_LOGGER
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| Arc::new(build_logger()));
    }
}

/// End the active lifetime: take the logger out of the slot and close it.
///
/// The write lock is what makes this safe. It is granted only once no emission
/// holds the read lock, so the logger being closed has no writer, and every
/// later emitter finds the slot empty and builds a fresh one.
fn retire_logger() -> Result<(), LoggerError> {
    let retired = ORES_LOGGER
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    match retired {
        Some(logger) => logger.close(),
        None => Ok(()),
    }
}

/// Ends the telemetry lifetime when dropped. Holds nothing: the slot owns the
/// logger, so a guard cannot keep a closed one alive.
pub struct TelemetryGuard {
    _not_constructible_elsewhere: (),
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if retire_logger().is_err() {
            eprintln!("telemetry: Ores logger shutdown failed; final records may be incomplete");
        }
    }
}

pub fn init() -> TelemetryGuard {
    let filter = canonical_api_server::flags::var("RUST_LOG")
        .ok()
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_ansi(false)
        .with_target(true)
        .init();

    let _ = with_ores_logger(|logger| {
        logger
            .info(vec![json!("telemetry initialized")])
            .add_fields(JsonObject::from_iter([
                ("service.name".to_string(), json!(SERVICE_NAME)),
                ("service.namespace".to_string(), json!(SERVICE_NAMESPACE)),
                ("log.destination".to_string(), json!("tracing-bridge")),
            ]))
            .send()
    });
    tracing::info!(
        service.name = SERVICE_NAME,
        service.namespace = SERVICE_NAMESPACE,
        log.format = "json",
        log.destination = "stderr",
        "telemetry initialized"
    );

    TelemetryGuard {
        _not_constructible_elsewhere: (),
    }
}

/// Emits an RPC operation failure through the ores-otel (next-loggers) seam.
///
/// Callers trap the error, call this, and then re-raise the error unchanged;
/// nothing here may alter the RPC result. Two properties make that safe:
///
/// * **Payload-free.** Only the operation key, the service's own stable error
///   code, and the caller's static ids are emitted. Request or response bodies,
///   headers, bearer tokens, user ids, directory records, paths and query
///   values are never passed in and never logged.
/// * **Fail-open.** `send()` already returns a `Result` that is discarded, and
///   the whole emit is unwind-guarded so a panicking transport cannot escape
///   into the RPC dispatch.
///
/// `trace_id` and `routine_id` are always inline `ores-trace-` /
/// `ores-routine-` literals supplied by the call site; this function never
/// mints or assembles an id.
pub(crate) fn log_rpc_error(
    operation_key: &str,
    error_code: &str,
    trace_id: &'static str,
    routine_id: &'static str,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_ores_logger(|logger| {
            let _ = logger
                .error(vec![json!("rpc operation failed")])
                .add_fields(JsonObject::from_iter([
                    ("service.name".to_string(), json!(SERVICE_NAME)),
                    ("service.namespace".to_string(), json!(SERVICE_NAMESPACE)),
                    ("rpc.system".to_string(), json!("ores.rpc.v1")),
                    ("rpc.operation".to_string(), json!(operation_key)),
                    ("rpc.error_code".to_string(), json!(error_code)),
                ]))
                .add_trace(trace_id, false)
                .add_routine_id(routine_id)
                .send();
        });
    }));
}

struct TracingBridgeTransport;

/// Is anything listening on the tracing side of the bridge?
///
/// Checks the CURRENT dispatcher, so a scoped subscriber counts as well as the
/// global one.
fn tracing_is_listening() -> bool {
    tracing::dispatcher::get_default(|dispatch| !dispatch.is::<tracing::subscriber::NoSubscriber>())
}

impl Transport for TracingBridgeTransport {
    fn write(&self, record: &LogRecord) -> Result<(), LoggerError> {
        let encoded = record.to_json()?;
        if !tracing_is_listening() {
            // An error before init(), or in a process that never calls it. The
            // logger is built with console off and this bridge as its only
            // transport, so with no subscriber the record went nowhere at all,
            // which is not the fallback the doc comment used to promise. The
            // record is payload-free by construction, so stderr is safe.
            eprintln!("{encoded}");
            return Ok(());
        }
        match record.level {
            LogLevel::Trace => tracing::trace!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Debug => tracing::debug!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Info => tracing::info!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Warn => tracing::warn!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Error => tracing::error!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Fatal => {
                tracing::error!(ores.record = %encoded, "Ores fatal structured log")
            }
        }
        Ok(())
    }

    fn is_open_telemetry(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    /// The slot and the build counter are process-wide, so lifecycle tests
    /// cannot overlap.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("capture").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("capture")).into_owned()
        }
        /// Emitted EVENTS mentioning `needle`: one JSON line each. A record
        /// carries its message more than once, so counting substrings counts
        /// fields, not events.
        fn count(&self, needle: &str) -> usize {
            self.text()
                .lines()
                .filter(|line| line.contains(needle))
                .count()
        }
    }

    /// Run `body` with a scoped JSON subscriber standing in for the one init()
    /// installs globally (which can only be installed once per process).
    fn with_subscriber<T>(body: impl FnOnce(&Captured) -> T) -> T {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || body(&captured))
    }

    fn an_error(operation: &str) {
        log_rpc_error(
            operation,
            "rpc.dispatch_failed",
            "ores-trace-uXOkCvZNc9WidUlNR7Pvn",
            "ores-routine-JJkmmS1t9iAh6taE0Hj1I",
        );
    }

    fn a_guard() -> TelemetryGuard {
        // What init() returns, without installing the once-only global
        // subscriber.
        with_ores_logger(|_| ());
        TelemetryGuard {
            _not_constructible_elsewhere: (),
        }
    }

    #[test]
    fn the_logger_is_built_once_per_active_lifetime() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = retire_logger();
        with_subscriber(|captured| {
            let before = LOGGER_BUILDS.load(Ordering::Relaxed);
            let guard = a_guard();
            for n in 0..50 {
                an_error(&format!("demo.users.find_{n}"));
            }
            assert_eq!(
                LOGGER_BUILDS.load(Ordering::Relaxed) - before,
                1,
                "fifty failed calls must share one logger and one transport"
            );
            assert_eq!(captured.count("rpc operation failed"), 50);
            drop(guard);
        });
    }

    #[test]
    fn a_late_error_after_shutdown_is_still_logged() {
        // The defect: OnceLock kept the CLOSED logger forever, so this record
        // was written into a closed logger and lost without a word.
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = retire_logger();
        with_subscriber(|captured| {
            let guard = a_guard();
            an_error("demo.users.before_shutdown");
            drop(guard);
            assert!(
                ORES_LOGGER
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_none(),
                "the guard must retire the logger, not leave a closed one behind"
            );

            an_error("demo.users.after_shutdown");
            let text = captured.text();
            assert!(text.contains("demo.users.before_shutdown"), "{text}");
            assert!(
                text.contains("demo.users.after_shutdown"),
                "a late error vanished: {text}"
            );
        });
        let _ = retire_logger();
    }

    #[test]
    fn telemetry_can_be_started_again() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = retire_logger();
        with_subscriber(|captured| {
            let before = LOGGER_BUILDS.load(Ordering::Relaxed);
            for lifetime in 0..3 {
                let guard = a_guard();
                an_error(&format!("demo.lifetime.number_{lifetime}"));
                an_error(&format!("demo.lifetime.number_{lifetime}"));
                drop(guard);
            }
            assert_eq!(
                LOGGER_BUILDS.load(Ordering::Relaxed) - before,
                3,
                "one logger per lifetime: not one forever, and not one per call"
            );
            for lifetime in 0..3 {
                assert_eq!(
                    captured.count(&format!("demo.lifetime.number_{lifetime}")),
                    2,
                    "lifetime {lifetime} lost records"
                );
            }
        });
    }

    /// The race, made deterministic. An emitter is stopped INSIDE its emission —
    /// past the point where it obtained the logger, before it writes — and
    /// retirement is started while it sits there.
    ///
    /// When emitters were handed an `Arc<Logger>` and the lock released,
    /// retirement completed at once, closed the logger under the emitter, and
    /// the emitter's write panicked ("logger is closed"): the closed-logger
    /// failure, moved from after shutdown to during it.
    #[test]
    fn retirement_waits_for_an_emission_already_in_flight() {
        use std::sync::mpsc;
        use std::time::Duration;

        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = retire_logger();
        with_ores_logger(|_| ()); // an active lifetime to retire

        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (retired_tx, retired_rx) = mpsc::channel::<()>();

        let emitter = std::thread::spawn(move || {
            with_subscriber(|captured| {
                let wrote = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    with_ores_logger(|logger| {
                        entered_tx.send(()).expect("signal entered");
                        release_rx.recv().expect("wait for release");
                        let _ = logger.error(vec![json!("written mid-retirement")]).send();
                    });
                }));
                (wrote.is_ok(), captured.count("written mid-retirement"))
            })
        });

        entered_rx
            .recv()
            .expect("the emitter is inside its emission");
        let retirer = std::thread::spawn(move || {
            let result = retire_logger();
            retired_tx.send(()).expect("signal retired");
            result
        });

        // The emitter holds the read lock and is going nowhere until released,
        // so retirement must still be waiting. A wait that can only time out is
        // the one place a sleep is the honest tool: the assertion is that
        // something does NOT happen.
        assert!(
            retired_rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "the logger was retired under an emitter that had not written yet"
        );

        release_tx.send(()).expect("release the emitter");
        let (wrote_without_panic, delivered) = emitter.join().expect("emitter thread");
        retirer.join().expect("retirer thread").expect("close");
        retired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("retirement completes once the emission has");

        assert!(wrote_without_panic, "the emitter wrote to a closed logger");
        assert_eq!(delivered, 1, "the in-flight record was lost");
        assert!(ORES_LOGGER
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none());
    }

    /// The premise of the tests above, checked against the pinned next-loggers
    /// rather than assumed. A closed logger does not quietly drop records: it
    /// PANICS ("next_loggers: logger is closed"). So under the OnceLock design a
    /// late RPC error panicked inside log_rpc_error, where catch_unwind
    /// swallowed it — the record was lost and the only trace was a panic
    /// message on stderr. If this ever stops holding, revisit the retire design.
    #[test]
    fn using_a_closed_logger_panics_which_is_why_it_is_retired() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        with_subscriber(|captured| {
            let logger = build_logger();
            let _ = logger.info(vec![json!("before close")]).send();
            logger.close().expect("close");
            let after = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = logger.info(vec![json!("after close")]).send();
            }));
            assert_eq!(captured.count("before close"), 1);
            assert!(
                after.is_err(),
                "next-loggers no longer panics on a closed logger"
            );
            assert_eq!(captured.count("after close"), 0);
        });
    }

    #[test]
    fn an_error_before_init_has_somewhere_to_go() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        // No subscriber: the bridge must notice and use stderr instead of
        // emitting a tracing event nobody receives.
        assert!(
            !tracing_is_listening(),
            "the premise: nothing is installed here"
        );
        with_subscriber(|_| assert!(tracing_is_listening()));
        // And the emit itself must not panic or block without one.
        let _ = retire_logger();
        an_error("demo.users.before_init");
        let _ = retire_logger();
    }
}
