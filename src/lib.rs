mod auth;
mod config;
// `quote` imports `SinkExt` for compatibility with alternate WebSocket sink implementations;
// Axum 0.8 exposes an inherent `send` method, so the pinned toolchain sees it as unused.
#[allow(unused_imports)]
mod quote;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbErr,
    Statement,
};
use serde_json::json;
use thiserror::Error;
use tokio::sync::{broadcast, Semaphore};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

pub use config::Config;
pub(crate) use quote::QuoteEvent;

pub const SERVICE_NAME: &str = "canonical-api-server";

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) db: DatabaseConnection,
    pub(crate) http: reqwest::Client,
    pub(crate) events: broadcast::Sender<QuoteEvent>,
    pub(crate) websocket_capacity: Arc<Semaphore>,
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("authentication required")]
    Unauthorized,
    #[error("request is forbidden")]
    Forbidden,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("resource not found")]
    NotFound,
    #[error("server capacity is temporarily exhausted")]
    Busy,
    #[error("database error")]
    Database(#[from] DbErr),
    #[error("upstream service is unavailable")]
    Upstream,
    #[error("serialization failed")]
    Serialization(#[from] serde_json::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required",
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "request is forbidden"),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message.as_str()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "resource not found"),
            Self::Busy => (
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_exhausted",
                "server capacity is temporarily exhausted",
            ),
            Self::Upstream => (
                StatusCode::SERVICE_UNAVAILABLE,
                "upstream_unavailable",
                "an upstream service is temporarily unavailable",
            ),
            Self::Database(_) | Self::Serialization(_) => {
                tracing::error!(error = %self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "the server could not complete the request",
                )
            }
        };
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

pub async fn run() -> anyhow::Result<()> {
    init_tracing();
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some(other) => anyhow::bail!("unknown command {other:?}; expected serve or migrate"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env().context("loading configuration")?;
    let db = connect_database(&config.database_url, config.database_max_connections)
        .await
        .context("connecting runtime database")?;
    db.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT 1 AS ready",
    ))
    .await
    .context("checking runtime database")?;

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .user_agent("canonical-api-server/0.1")
        .build()
        .context("building outbound HTTP client")?;
    let (events, _) = broadcast::channel(512);
    let state = AppState {
        websocket_capacity: Arc::new(Semaphore::new(config.websocket_max_connections)),
        config: Arc::new(config),
        db,
        http,
        events,
    };
    let _workers = quote::spawn_workers(state.clone());

    let request_id = HeaderName::from_static("x-request-id");
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(quote::router())
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            HeaderName::from_static("x-canonical-service-token"),
        ]))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http());

    let bind = format!("0.0.0.0:{}", state.config.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(
        bind,
        model = %state.config.gemini_model,
        workers = state.config.worker_concurrency,
        "canonical API listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

async fn migrate() -> anyhow::Result<()> {
    let database_url = std::env::var("MIGRATION_DATABASE_URL")
        .context("MIGRATION_DATABASE_URL is required for migrate")?;
    let db = connect_database(&database_url, 2)
        .await
        .context("connecting migration database")?;
    db.execute_unprepared(include_str!("../db/schema.sql"))
        .await
        .context("applying quote schema")?;
    tracing::info!("canonical API schema applied");
    Ok(())
}

async fn connect_database(url: &str, max_connections: u32) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(url.to_owned());
    options
        .max_connections(max_connections)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(5))
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false);
    Database::connect(options).await
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn readyz(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT 1 AS ready",
        ))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init();
}
