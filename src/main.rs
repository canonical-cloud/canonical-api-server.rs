#![forbid(unsafe_code)]

mod readiness;
mod shutdown;
mod telemetry;

use std::{io, time::Duration};

use canonical_api_server::{
    build_router, AppState, Config, GeminiClient, WebhookDispatcher, SHARED_AUTH_MAX_RESPONSE_BYTES,
};
use sea_orm::Database;
use shared_auth_client::SharedAuthClient;
use tokio::net::TcpListener;
use tracing::info;

fn shutdown_grace() -> Duration {
    const DEFAULT_MS: u64 = 30_000;
    let milliseconds = std::env::var("SHUTDOWN_GRACE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MS);
    Duration::from_millis(milliseconds)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry = telemetry::init();

    let config = Config::from_env()?;
    let database = match config.database_url.as_deref() {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let readiness_database = database.clone();
    let database_configured = database.is_some();
    let gemini_configured = config.gemini_api_key.is_some();

    let listener = TcpListener::bind(&config.bind_address).await?;
    let mut state = AppState::new(
        config.internal_auth_token,
        config.gemini_model.clone(),
        database,
    );
    if let Some(api_key) = config.gemini_api_key {
        state = state.with_gemini(GeminiClient::new(api_key, config.gemini_model.clone())?);
    }
    if let (Some(endpoint), Some(secret)) = (config.webhook_endpoint, config.webhook_secret) {
        state = state.with_webhook(WebhookDispatcher::new(&endpoint, secret)?);
    }
    if let (Some(base), Some(secret)) = (
        config.shared_auth_base,
        config.shared_auth_introspect_secret,
    ) {
        let client = SharedAuthClient::try_new(base)?
            .with_service_credential(secret)
            .with_max_response_bytes(SHARED_AUTH_MAX_RESPONSE_BYTES);
        state = state.with_shared_auth(client, config.shared_auth_audience);
    }

    state.mark_started();
    let accepting = state.accepting_handle();
    let app = build_router(state).merge(readiness::router(readiness_database, accepting.clone()));
    info!(
        address = %config.bind_address,
        database_configured,
        gemini_configured,
        gemini_model = %config.gemini_model,
        "canonical API listening"
    );
    let outcome = shutdown::serve(
        listener,
        app,
        shutdown::Config {
            grace: shutdown_grace(),
            ..shutdown::Config::default()
        },
        accepting,
    )
    .await?;

    match outcome {
        shutdown::Outcome::Graceful => Ok(()),
        shutdown::Outcome::Forced(trigger) => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("server shutdown forced by {trigger:?}"),
        )
        .into()),
    }
}
