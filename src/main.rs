#![forbid(unsafe_code)]

mod readiness;
mod readiness_observation_ingest;
mod shutdown;
mod telemetry;

use std::{io, time::Duration};

use axum::{extract::Request, response::Response, Router};
use canonical_api_server::{
    build_router, AppState, Config, GeminiClient, WebhookDispatcher, SHARED_AUTH_MAX_RESPONSE_BYTES,
};
use canonical_lib::interfaces::QuoteRequest;
use ores_api_docs::RouteMap;
use sea_orm::Database;
use shared_auth_client::SharedAuthClient;
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing::info;

include!(concat!(env!("OUT_DIR"), "/ores_filesystem_api.rs"));

const FILESYSTEM_ROUTE_MAP: &str = include_str!("../contracts/filesystem-pilot.route-map.json");

#[derive(Clone)]
struct FilesystemRouteState {
    legacy: Router,
}

async fn forward_filesystem_request(state: FilesystemRouteState, request: Request) -> Response {
    match state.legacy.oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    }
}

fn shutdown_grace() -> Duration {
    const DEFAULT_MS: u64 = 30_000;
    let milliseconds = canonical_api_server::flags::var("SHUTDOWN_GRACE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MS);
    Duration::from_millis(milliseconds)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(output) =
        canonical_api_server::flags::process_control().map_err(io::Error::other)?
    {
        print!("{output}");
        return Ok(());
    }
    let _telemetry = telemetry::init();

    let config = Config::from_env()?;
    let database = match config.database_url.as_deref() {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let readiness_database = database.clone();
    let observation_service =
        readiness_observation_ingest::ObservationService::from_env(database.clone())?;
    let database_configured = database.is_some();
    let gemini_configured = config.gemini_api_key.is_some();
    let quote_request_contract = std::any::type_name::<QuoteRequest>();

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

    let legacy = build_router(state)
        .merge(readiness::router(readiness_database))
        .merge(readiness_observation_ingest::router(observation_service));
    let filesystem_state = FilesystemRouteState {
        legacy: legacy.clone(),
    };
    let route_map = RouteMap::from_json_str(FILESYSTEM_ROUTE_MAP)?;
    let filesystem = __ores_filesystem_api_http_and_rpc_router!(filesystem_state, route_map)?;
    let app = filesystem.fallback_service(legacy);

    info!(
        address = %config.bind_address,
        database_configured,
        gemini_configured,
        gemini_model = %config.gemini_model,
        quote_request_contract,
        "canonical API listening"
    );
    let outcome = shutdown::serve(
        listener,
        app,
        shutdown::Config {
            grace: shutdown_grace(),
            ..shutdown::Config::default()
        },
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
