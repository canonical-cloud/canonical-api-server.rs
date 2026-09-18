//! Ores structured logging bridged into the service's JSON tracing stream.
//!
//! The bridge installs one subscriber and never attaches credentials, URLs,
//! request bodies, identity values, or upstream response bodies.

use std::sync::Arc;

use next_loggers::{
    json, JsonObject, LogLevel, LogRecord, Logger, LoggerError, Options, Transport,
};
use tracing_subscriber::EnvFilter;

const SERVICE_NAME: &str = "canonical-api-server";
const SERVICE_NAMESPACE: &str = "canonical-cloud";

pub struct TelemetryGuard {
    ores_logger: Logger,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if self.ores_logger.close().is_err() {
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

    let ores_logger = Logger::new(Options {
        app_name: SERVICE_NAME.to_string(),
        name: Some("server".to_string()),
        console: false,
        transports: vec![Arc::new(TracingBridgeTransport)],
        ..Options::default()
    });
    let _ = ores_logger
        .info(vec![json!("telemetry initialized")])
        .add_fields(JsonObject::from_iter([
            ("service.name".to_string(), json!(SERVICE_NAME)),
            ("service.namespace".to_string(), json!(SERVICE_NAMESPACE)),
            ("log.destination".to_string(), json!("tracing-bridge")),
        ]))
        .send();
    tracing::info!(
        service.name = SERVICE_NAME,
        service.namespace = SERVICE_NAMESPACE,
        log.format = "json",
        log.destination = "stderr",
        "telemetry initialized"
    );

    TelemetryGuard { ores_logger }
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
        let logger = Logger::new(Options {
            app_name: SERVICE_NAME.to_string(),
            name: Some("server".to_string()),
            console: false,
            transports: vec![Arc::new(TracingBridgeTransport)],
            ..Options::default()
        });
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
    }));
}

struct TracingBridgeTransport;

impl Transport for TracingBridgeTransport {
    fn write(&self, record: &LogRecord) -> Result<(), LoggerError> {
        let encoded = record.to_json()?;
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
