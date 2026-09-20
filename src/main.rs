#![forbid(unsafe_code)]

mod readiness;
mod readiness_observation_ingest;
mod rpc_routes;
mod shutdown;
mod telemetry;

use std::{env, io, time::Duration};

use canonical_api_server::{
    build_router, AppState, Config, GeminiClient, WebhookDispatcher, SHARED_AUTH_MAX_RESPONSE_BYTES,
};
use canonical_lib::{audit_data::table, interfaces::QuoteRequest};
use canonical_orm_core::{CapabilityProfile, DualOrmContext, QuoteStore};
use shared_auth_client::SharedAuthClient;
use tokio::net::TcpListener;
use tracing::info;

const ADMIN_DATABASE_URL_ENV: &str = "CANONICAL_ADMIN_DATABASE_URL";
const AUDIT_DATABASE_URL_ENV: &str = "CANONICAL_AUDIT_DATABASE_URL";

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

    if env::var_os(ADMIN_DATABASE_URL_ENV).is_some() {
        return Err(io::Error::other(
            "CANONICAL_ADMIN_DATABASE_URL belongs to the isolated admin plane and is forbidden in the customer API",
        )
        .into());
    }

    let quote_database_url = match config.database_url.as_deref() {
        Some(value) if value.trim().is_empty() => {
            return Err(io::Error::other("DATABASE_URL is configured but empty").into());
        }
        Some(value) => Some(value.trim()),
        None => None,
    };
    let quote_database_configured = quote_database_url.is_some();

    // A completely database-free process remains a supported health/dev mode.
    // Once quote/readiness persistence is configured, the separate audit plane
    // must be configured and verified too. Neither credential is ever exposed
    // as a raw driver handle to application state.
    let audit_database_url = match env::var(AUDIT_DATABASE_URL_ENV) {
        Ok(value) if value.trim().is_empty() => {
            return Err(io::Error::other("CANONICAL_AUDIT_DATABASE_URL must not be empty").into());
        }
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) if quote_database_configured => {
            return Err(io::Error::other(
                "CANONICAL_AUDIT_DATABASE_URL is required whenever quote/readiness DATABASE_URL is configured",
            )
            .into());
        }
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(io::Error::other(
                "CANONICAL_AUDIT_DATABASE_URL is configured but is not valid UTF-8",
            )
            .into());
        }
    };

    let mut dual_orm_verified = false;
    if let Some(audit_database_url) = audit_database_url.as_deref() {
        let audit_database_url = audit_database_url.trim();
        if quote_database_url.is_some_and(|quote_url| quote_url == audit_database_url) {
            return Err(io::Error::other(
                "CANONICAL_AUDIT_DATABASE_URL must not equal quote/readiness DATABASE_URL",
            )
            .into());
        }

        let dual_orm =
            DualOrmContext::connect_read_write(audit_database_url, CapabilityProfile::ApiReadWrite)
                .await?;
        dual_orm.ping_both().await?;
        dual_orm.assert_catalog_congruence().await?;
        dual_orm_verified = true;
    }

    // QuoteStore owns both SeaORM and Diesel handles and certifies the complete
    // quote runtime role/schema/RLS/grant contract before it can escape.
    let quote_store = match quote_database_url {
        Some(url) => Some(QuoteStore::connect(url).await?),
        None => None,
    };
    let readiness_store = quote_store.clone();
    let observation_service =
        readiness_observation_ingest::ObservationService::from_env(quote_store.clone())?;
    let database_configured = quote_store.is_some();
    let gemini_configured = config.gemini_api_key.is_some();
    let quote_request_contract = std::any::type_name::<QuoteRequest>();

    let listener = TcpListener::bind(&config.bind_address).await?;
    let mut state = AppState::new(
        config.internal_auth_token,
        config.gemini_model.clone(),
        quote_store,
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
        .merge(readiness::router(readiness_store))
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
