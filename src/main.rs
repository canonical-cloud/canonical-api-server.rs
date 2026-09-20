#![forbid(unsafe_code)]

mod readiness;
mod readiness_observation_ingest;
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

fn audit_database_url(customer_database_url: Option<&str>) -> Result<Option<String>, io::Error> {
    if canonical_api_server::flags::var(ADMIN_DATABASE_URL_ENV).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CANONICAL_ADMIN_DATABASE_URL belongs to the isolated admin plane and is forbidden in the customer API process",
        ));
    }
    let configured = canonical_api_server::flags::var(AUDIT_DATABASE_URL_ENV).ok();
    validate_audit_database_url(customer_database_url, configured.as_deref())
        .map(|value| value.map(str::to_owned))
}

fn validate_audit_database_url<'a>(
    customer_database_url: Option<&str>,
    audit_database_url: Option<&'a str>,
) -> Result<Option<&'a str>, io::Error> {
    match audit_database_url {
        None if customer_database_url.is_some() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CANONICAL_AUDIT_DATABASE_URL is required when DATABASE_URL is configured; the application store and audit-plane capability credentials must remain separate",
        )),
        None => Ok(None),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "CANONICAL_AUDIT_DATABASE_URL must not be empty",
                ));
            }
            if customer_database_url.is_some_and(|customer| customer.trim() == value) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "CANONICAL_AUDIT_DATABASE_URL must not reuse DATABASE_URL; the quote/readiness store and audit-plane capability login are separate trust boundaries",
                ));
            }
            Ok(Some(value))
        }
    }
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
    let audit_database_url = audit_database_url(config.database_url.as_deref())?;
    let dual_orm = match audit_database_url.as_deref() {
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

#[cfg(test)]
mod tests {
    use super::validate_audit_database_url;

    #[test]
    fn audit_plane_credential_is_separate_from_application_store() {
        assert!(validate_audit_database_url(Some("postgres://app@db/app"), None).is_err());
        assert!(validate_audit_database_url(None, None).unwrap().is_none());
        assert!(
            validate_audit_database_url(
                Some("postgres://app@db/app"),
                Some("postgres://app@db/app")
            )
            .is_err()
        );
        assert_eq!(
            validate_audit_database_url(
                Some("postgres://app@db/app"),
                Some(" postgres://audit_rw@db/audit ")
            )
            .unwrap(),
            Some("postgres://audit_rw@db/audit")
        );
        assert_eq!(
            validate_audit_database_url(None, Some("postgres://audit_rw@db/audit")).unwrap(),
            Some("postgres://audit_rw@db/audit")
        );
    }
}
