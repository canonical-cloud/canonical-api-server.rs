#![forbid(unsafe_code)]

mod readiness;
mod readiness_observation_ingest;
#[path = "routes/mod.rs"]
mod rpc_routes;
mod shutdown;
mod telemetry;

use std::{io, time::Duration};

use canonical_api_server::{
    build_router, AppState, Config, GeminiClient, WebhookDispatcher, SHARED_AUTH_MAX_RESPONSE_BYTES,
};
use canonical_lib::{audit_data::table, interfaces::QuoteRequest};
use canonical_orm_core::{CapabilityProfile, DualOrmContext};
use sea_orm::Database;
use shared_auth_client::SharedAuthClient;
use tokio::net::TcpListener;
use tracing::info;

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
    let dual_orm = match config.database_url.as_deref() {
        Some(url) => {
            let context = DualOrmContext::connect_read_write(url, CapabilityProfile::ApiReadWrite)
                .await?;
            context.ping_both().await?;
            context.assert_catalog_congruence().await?;
            Some(context)
        }
        None => None,
    };
    let database = match config.database_url.as_deref() {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let readiness_database = database.clone();
    let observation_service =
        readiness_observation_ingest::ObservationService::from_env(database.clone())?;
    let database_configured = database.is_some();
    let dual_orm_verified = dual_orm.is_some();
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

    let app = build_router(state.clone())
        .merge(rpc_routes::router(state))
        .merge(readiness::router(readiness_database))
        .merge(readiness_observation_ingest::router(observation_service));
    info!(
        address = %config.bind_address,
        database_configured,
        dual_orm_verified,
        tenant_table = table::TENANTS,
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
