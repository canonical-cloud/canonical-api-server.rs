#![forbid(unsafe_code)]

use canonical_api_server::{
    build_router, spawn_notification_worker, spawn_quote_worker, AppState, Config, GeminiClient,
    TwilioMessagingClient,
};
use sea_orm::Database;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    let config = Config::from_env()?;
    let database = match config.database_url.as_deref() {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let worker_database = match config.worker_database_url.as_deref() {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let twilio = TwilioMessagingClient::from_env()?;
    let database_configured = database.is_some();
    let gemini_configured = config.gemini_api_key.is_some();
    let notification_worker_configured = worker_database.is_some() && twilio.is_some();
    let quote_worker_configured = worker_database.is_some();

    let listener = TcpListener::bind(&config.bind_address).await?;
    let mut state = AppState::new(
        config.internal_auth_token.clone(),
        config.gemini_model.clone(),
        database,
    )
    .with_quote_link_base_url(config.quote_link_base_url.clone());
    if let Some(key) = config.quote_data_encryption_key.as_deref() {
        state = state.with_quote_cipher_base64(key)?;
    }
    if let Some(api_key) = config.gemini_api_key.clone() {
        state = state.with_gemini(GeminiClient::new(api_key, config.gemini_model.clone())?);
    }
    if let Some(worker_database) = worker_database {
        spawn_quote_worker(state.clone(), worker_database.clone());
        if let Some(twilio) = twilio {
            spawn_notification_worker(state.clone(), worker_database, twilio);
        }
    }

    info!(
        address = %config.bind_address,
        database_configured,
        gemini_configured,
        quote_worker_configured,
        notification_worker_configured,
        gemini_model = %config.gemini_model,
        "canonical API listening"
    );
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
