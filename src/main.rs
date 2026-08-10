#![forbid(unsafe_code)]

mod readiness;

use canonical_api_server::{build_router, AppState, Config, GeminiClient};
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

    let app = build_router(state).merge(readiness::router(readiness_database));
    info!(
        address = %config.bind_address,
        database_configured,
        gemini_configured,
        gemini_model = %config.gemini_model,
        "canonical API listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}
