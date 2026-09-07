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

impl TelemetryGuard {
    /// Reuse the same transports and shutdown owner for browser/server context.
    /// This does not install another global subscriber or exporter.
    pub fn instrument(&self, router: axum::Router) -> axum::Router {
        ores_otel_web::server::install_with_logger(router, self.ores_logger.clone())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get, Router};
    use ores_otel_web::TraceParent;
    use std::sync::Mutex;
    use tower::ServiceExt;

    #[derive(Default)]
    struct Capture(Mutex<Vec<LogRecord>>);
    impl Transport for Capture {
        fn write(&self, record: &LogRecord) -> Result<(), LoggerError> {
            self.0.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn browser_context_reaches_the_existing_application_transport() {
        let capture = Arc::new(Capture::default());
        let telemetry = TelemetryGuard {
            ores_logger: Logger::new(Options {
                app_name: SERVICE_NAME.into(),
                console: false,
                transports: vec![capture.clone()],
                ..Options::default()
            }),
        };
        let app = telemetry.instrument(Router::new().route("/", get(|| async { "ok" })));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?secret=never-record")
                    .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let trace: TraceParent = response.headers()["traceparent"].to_str().unwrap().parse().unwrap();
        let records = capture.0.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].trace_id.as_deref(), Some(trace.trace_id()));
        assert!(!records[0].to_json().unwrap().contains("never-record"));
        assert!(!trace.sampled());
    }
}
