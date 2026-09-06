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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
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
