#![forbid(unsafe_code)]

use canonical_api_server::{build_router, AppState, Config};
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
    let listener = TcpListener::bind(&config.bind_address).await?;
    let state = AppState::new(config.internal_auth_token, config.gemini_model, database);

    info!(address = %config.bind_address, "canonical API listening");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
