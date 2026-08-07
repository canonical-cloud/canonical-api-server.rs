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

mod auth;
mod gemini;
mod persistence;
mod wire;

use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tokio::sync::{broadcast, RwLock};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, warn};
use uuid::Uuid;

pub use gemini::{GeminiBuildError, GeminiClient};

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-pro-preview";
const MAX_MARKDOWN_CONTEXT_BYTES: usize = 262_144;
const MAX_NOTES_BYTES: usize = 32_768;
const MAX_CONTEXT_RECORD_BYTES: usize = 262_144;
const APPLICATION_CONTEXT_MARKDOWN: &str = include_str!("../context/quote-analysis.md");
const ALLOWED_FRAMEWORKS: &[&str] = &[
    "custom",
    "fedramp",
    "gdpr",
    "hipaa",
    "iso_27001",
    "nist_800_53",
    "nist_csf_2",
    "pci_dss_4",
    "soc2_type_1",
    "soc2_type_2",
];

#[derive(Clone)]
pub struct Config {
    pub bind_address: String,
    pub database_url: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,
    pub internal_auth_token: String,
    pub shared_auth_verify_url: Option<String>,
}

impl Config {
    fn from_env() -> Result<Self, ApiError> {
        let bind_addr = env_or("BIND_ADDR", "0.0.0.0:8081")
            .parse()
            .map_err(|_| ApiError::Config("BIND_ADDR"))?;
        let database_url = required_env("DATABASE_URL")?;
        let database_max_connections = env_or("DATABASE_MAX_CONNECTIONS", "10")
            .parse()
            .map_err(|_| ApiError::Config("DATABASE_MAX_CONNECTIONS"))?;
        if !(1..=50).contains(&database_max_connections) {
            return Err(ApiError::Config("DATABASE_MAX_CONNECTIONS"));
        }
        let gemini_api_key = required_env("GEMINI_API_KEY")?;
        let gemini_model = env_or("GEMINI_MODEL", "gemini-3.1-pro-preview");
        if !gemini_model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(ApiError::Config("GEMINI_MODEL"));
        }
        let quote_context_path = PathBuf::from(env_or(
            "QUOTE_CONTEXT_MARKDOWN_PATH",
            "context/quote-analysis.md",
        ));
        let origin_assertion_secret = required_env("ORIGIN_ASSERTION_SECRET")?.into_bytes();
        if origin_assertion_secret.len() < 32 {
            return Err(ApiError::Config("ORIGIN_ASSERTION_SECRET"));
        }
        let assertion_max_age_seconds = env_or("ORIGIN_ASSERTION_MAX_AGE_SECONDS", "30")
            .parse()
            .map_err(|_| ApiError::Config("ORIGIN_ASSERTION_MAX_AGE_SECONDS"))?;
        if !(5..=60).contains(&assertion_max_age_seconds) {
            return Err(ApiError::Config("ORIGIN_ASSERTION_MAX_AGE_SECONDS"));
        }

        let gemini_api_key = env::var("GEMINI_API_KEY").ok();
        if gemini_api_key
            .as_deref()
            .is_some_and(|key| key.trim().len() < 20)
        {
            return Err(ConfigError::WeakGeminiApiKey);
        }

        let gemini_model = env::var("GEMINI_MODEL").unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.into());
        if !is_valid_model_name(&gemini_model) {
            return Err(ConfigError::InvalidGeminiModel);
        }

        Ok(Self {
            bind_address: env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: env::var("DATABASE_URL").ok(),
            gemini_api_key,
            gemini_model,
            internal_auth_token,
            shared_auth_verify_url: env::var("SHARED_AUTH_VERIFY_URL").ok(),
        })
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind_address", &self.bind_address)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "gemini_api_key",
                &self.gemini_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field("gemini_model", &self.gemini_model)
            .field("internal_auth_token", &"[redacted]")
            .field("shared_auth_verify_url", &self.shared_auth_verify_url)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    InvalidGeminiModel,
    InvalidSharedAuthVerifyUrl,
    MissingInternalAuthToken,
    WeakGeminiApiKey,
    WeakInternalAuthToken,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeminiModel => formatter.write_str(
                "GEMINI_MODEL must contain only ASCII letters, digits, '.', '-', or '_'",
            ),
            Self::InvalidSharedAuthVerifyUrl => formatter.write_str(
                "SHARED_AUTH_VERIFY_URL must be the exact HTTPS /shared-auth/auth/verify endpoint, except on loopback or Kubernetes service DNS",
            ),
            Self::MissingInternalAuthToken => {
                formatter.write_str("CANONICAL_INTERNAL_AUTH_TOKEN is required")
            }
            Self::WeakGeminiApiKey => {
                formatter.write_str("GEMINI_API_KEY is present but is unexpectedly short")
            }
            Self::WeakInternalAuthToken => formatter.write_str(
                "CANONICAL_INTERNAL_AUTH_TOKEN must contain at least 32 non-whitespace bytes",
            ),
        }
        validate_list(&mut self.data_types, 12, 80)?;
        validate_list(&mut self.cloud_providers, 12, 80)?;
        Ok(())
    }
}

fn bounded_text(value: &str, minimum: usize, maximum: usize) -> Result<String, ApiError> {
    let output = value.trim().to_owned();
    let count = output.chars().count();
    if count < minimum || count > maximum || output.contains('\0') {
        return Err(ApiError::Validation("text field"));
    }
    Ok(output)
}

fn validate_list(
    values: &mut Vec<String>,
    maximum_items: usize,
    maximum_chars: usize,
) -> Result<(), ApiError> {
    if values.len() > maximum_items {
        return Err(ApiError::Validation("list field"));
    }
    for value in values.iter_mut() {
        *value = bounded_text(value, 1, maximum_chars)?;
    }
    values.sort();
    values.dedup();
    Ok(())
}

fn is_valid_model_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[derive(Clone)]
pub struct AppState {
    database: Option<DatabaseConnection>,
    events: broadcast::Sender<QuoteEvent>,
    gemini: Option<GeminiClient>,
    gemini_model: Arc<str>,
    auth: auth::Authenticator,
    event_sequence: Arc<AtomicU64>,
    quotes: Arc<RwLock<HashMap<Uuid, QuoteRecord>>>,
}

impl AppState {
    #[must_use]
    pub fn new(
        internal_auth_token: impl Into<Arc<str>>,
        gemini_model: impl Into<Arc<str>>,
        database: Option<DatabaseConnection>,
    ) -> Self {
        Self::try_new(internal_auth_token, gemini_model, database, None)
            .expect("static test authentication configuration")
    }

    pub fn try_new(
        internal_auth_token: impl Into<Arc<str>>,
        gemini_model: impl Into<Arc<str>>,
        database: Option<DatabaseConnection>,
        shared_auth_verify_url: Option<String>,
    ) -> Result<Self, ConfigError> {
        let (events, _) = broadcast::channel(256);
        Ok(Self {
            database,
            events,
            gemini: None,
            gemini_model: gemini_model.into(),
            auth: auth::Authenticator::new(internal_auth_token, shared_auth_verify_url)?,
            event_sequence: Arc::new(AtomicU64::new(0)),
            quotes: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    #[must_use]
    pub fn with_gemini(mut self, gemini: GeminiClient) -> Self {
        self.gemini = Some(gemini);
        self
    }
}

pub fn build_router(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/quotes", get(list_quotes).post(create_quote))
        .route("/api/v1/quotes/{quote_id}", get(get_quote))
        .route(
            "/api/v1/quotes/{quote_id}/retry",
            axum::routing::post(retry_quote),
        )
        .route("/api/v1/quotes/{quote_id}/events", get(quote_events))
        .with_state(state)
        .layer(SetSensitiveRequestHeadersLayer::new([
            axum::http::header::AUTHORIZATION,
            HeaderName::from_static("x-canonical-internal-token"),
        ]))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(90),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "canonical API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        database_configured: state.database.is_some(),
        gemini_configured: state.gemini.is_some(),
        gemini_model: state.gemini_model.to_string(),
        service: "canonical-api-server",
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn create_quote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<canonical_interfaces::QuoteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let subject = state.auth.authenticate(&headers).await?;
    let request = wire::normalize_request(request)?;

    let quote_id = Uuid::new_v4();
    let mut record = QuoteRecord {
        analysis: None,
        context_record_id: Uuid::nil(),
        error_code: None,
        frameworks: request.frameworks.clone(),
        gemini_model: state.gemini_model.to_string(),
        organization_name: request.organization.legal_name.clone(),
        owner_subject: subject.clone(),
        persistence: if state.database.is_some() {
            "postgres".into()
        } else {
            "memory-only".into()
        },
        quote_id,
        status: "queued".into(),
        request: request.wire.clone(),
        created_at: wire::now(),
        updated_at: wire::now(),
    };

    let context = match state.database.as_ref() {
        Some(database) => {
            let context = persistence::create_quote(database, &subject, &request, &record)
                .await
                .map_err(map_store_error)?;
            record.context_record_id = context.id;
            context
        }
        None => {
            let context = CanonicalContext::request_only();
            record.context_record_id = context.id;
            context
        }
    };
    let analysis_json = serde_json::to_value(&analysis).map_err(|_| ApiError::Internal)?;
    let updated_at = Utc::now();
    state
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE quote_request
               SET status = 'complete', analysis = $2, model = $3, updated_at = $4
               WHERE id = $1 AND owner_id = $5"#,
            [
                quote_id.into(),
                analysis_json.clone().into(),
                state.config.gemini_model.clone().into(),
                updated_at.into(),
                user.user_id.into(),
            ],
        ))
        .await?;
    emit(&state, user.user_id, quote_id, "complete");
    Ok((
        StatusCode::CREATED,
        Json(QuoteRecord {
            id: quote_id,
            owner_id: user.user_id,
            status: "complete".to_owned(),
            intake: intake_json,
            analysis: Some(analysis_json),
            model: Some(state.config.gemini_model.clone()),
            created_at: now,
            updated_at,
        }),
    ))
}

    state.quotes.write().await.insert(quote_id, record.clone());
    publish_event(&state, &record);

    let worker_state = state.clone();
    let worker_record = record.clone();
    tokio::spawn(async move {
        run_quote_analysis(worker_state, worker_record, request, context).await;
    });

    Ok((StatusCode::ACCEPTED, Json(wire::submission(&record))))
}

async fn run_quote_analysis(
    state: AppState,
    mut record: QuoteRecord,
    request: CreateQuoteRequest,
    context: CanonicalContext,
) {
    record.status = "analyzing".into();
    record.error_code = None;
    record.analysis = None;
    update_quote_state(&state, &record).await;

    let Some(gemini) = state.gemini.as_ref() else {
        record.status = "failed".into();
        record.error_code = Some("gemini_not_configured".into());
        update_quote_state(&state, &record).await;
        return;
    };

    let attempt_id = match state.database.as_ref() {
        Some(database) => match persistence::start_model_attempt(database, &record).await {
            Ok(attempt_id) => Some(attempt_id),
            Err(error) => {
                error!(
                    quote_id = %record.quote_id,
                    error_code = error.code(),
                    "failed to persist Gemini attempt start"
                );
                None
            }
        },
        None => None,
    };

    match gemini.analyze(&request, &context).await {
        Ok(analysis) => {
            record.status = "ready".into();
            record.analysis = Some(analysis);
            record.error_code = None;
        }
        Err(error) => {
            warn!(
                quote_id = %record.quote_id,
                error_code = error.code(),
                "Gemini quote analysis failed"
            );
            record.status = "failed".into();
            record.analysis = None;
            record.error_code = Some(error.code().into());
        }
    }

    update_quote_state(&state, &record).await;

    if let (Some(database), Some(attempt_id)) = (state.database.as_ref(), attempt_id) {
        if let Err(error) = persistence::finish_model_attempt(database, &record, attempt_id).await {
            error!(
                quote_id = %record.quote_id,
                error_code = error.code(),
                "failed to persist Gemini attempt completion"
            );
        }
    }
}

async fn update_quote_state(state: &AppState, record: &QuoteRecord) {
    let mut record = record.clone();
    record.updated_at = wire::now();
    state
        .quotes
        .write()
        .await
        .insert(record.quote_id, record.clone());

    if let Some(database) = state.database.as_ref() {
        if let Err(error) = persistence::update_quote(database, &record).await {
            error!(
                quote_id = %record.quote_id,
                error = %error,
                "failed to persist quote status"
            );
        }
    }

    publish_event(state, &record);
}

fn publish_event(state: &AppState, record: &QuoteRecord) {
    let sequence = state.event_sequence.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = state.events.send(QuoteEvent {
        owner_subject: record.owner_subject.clone(),
        quote_id: record.quote_id,
        status: record.status.clone(),
        sequence,
        occurred_at: record.updated_at,
        estimate: wire::detail(record).estimate,
        problem: wire::detail(record).problem,
    });
}

async fn list_quotes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<canonical_interfaces::QuoteListResponse>, ApiError> {
    let subject = state.auth.authenticate(&headers).await?;
    let records = match state.database.as_ref() {
        Some(database) => persistence::list_quotes(database, &subject, 50)
            .await
            .map_err(map_store_error)?,
        None => state
            .quotes
            .read()
            .await
            .values()
            .filter(|record| record.owner_subject == subject)
            .take(50)
            .cloned()
            .collect(),
    };
    Ok(Json(wire::list(records)))
}

async fn get_quote(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<canonical_interfaces::QuoteDetail>, ApiError> {
    let subject = state.auth.authenticate(&headers).await?;
    let record = find_quote(&state, quote_id, &subject).await?;
    Ok(Json(wire::detail(&record)))
}

async fn retry_quote(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<canonical_interfaces::QuoteRetryResponse>), ApiError> {
    let subject = state.auth.authenticate(&headers).await?;
    let mut record = find_quote(&state, quote_id, &subject).await?;
    if record.status != "failed" {
        return Err(ApiError::conflict(
            "quote_not_retryable",
            "only failed quotes may be retried",
        ));
    }
    record.status = "queued".into();
    record.analysis = None;
    record.error_code = None;
    record.updated_at = wire::now();
    update_quote_state(&state, &record).await;

    let request = wire::normalize_request(record.request.clone())?;
    let context = match state.database.as_ref() {
        Some(database) => persistence::get_context(database, &subject, record.context_record_id)
            .await
            .map_err(map_store_error)?,
        None => CanonicalContext::request_only(),
    };
    let response = wire::retry(&record);
    tokio::spawn(run_quote_analysis(state, record, request, context));
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn find_quote(
    state: &AppState,
    quote_id: Uuid,
    subject: &str,
) -> Result<QuoteRecord, ApiError> {
    if let Some(record) = state.quotes.read().await.get(&quote_id).cloned() {
        if record.owner_subject == subject {
            return Ok(record);
        }
        return Err(ApiError::not_found(
            "quote_not_found",
            "quote was not found",
        ));
    }

    let Some(database) = state.database.as_ref() else {
        return Err(ApiError::not_found(
            "quote_not_found",
            "quote was not found",
        ));
    };

    let record = persistence::get_quote(database, subject, quote_id)
        .await
        .map_err(map_store_error)?;
    state.quotes.write().await.insert(quote_id, record.clone());
    Ok(record)
}

async fn quote_events(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let subject = state.auth.authenticate(&headers).await?;
    find_quote(&state, quote_id, &subject).await?;

async fn quote_websocket(
    State(state): State<AppState>,
    user: EdgeUser,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| websocket_loop(socket, state, user.user_id))
}

async fn websocket_loop(socket: WebSocket, state: AppState, owner_id: Uuid) {
    let mut events = state.events.subscribe();
    if let Ok(record) = find_quote(&state, quote_id, &owner_subject).await {
        if send_json(&mut socket, &wire::detail(&record))
            .await
            .is_err()
        {
            return;
        }
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) if event.quote_id == quote_id
                        && event.owner_subject == owner_subject =>
                    {
                        if send_json(&mut socket, &wire::event(&event)).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn migrate() -> anyhow::Result<()> {
    let database_url = std::env::var("MIGRATION_DATABASE_URL")
        .context("MIGRATION_DATABASE_URL is required for migrate")?;
    let db = connect_database(&database_url, 2)
        .await
        .map_err(|_| ())
}

#[derive(Clone, Debug)]
pub struct CreateQuoteRequest {
    pub(crate) wire: canonical_interfaces::QuoteRequest,
    legacy_context_record_id: Option<Uuid>,
    pub frameworks: Vec<String>,
    pub markdown_context: String,
    pub notes: Option<String>,
    pub organization: OrganizationInput,
    pub target_date: Option<String>,
}

impl CreateQuoteRequest {
    fn validate_and_normalize(mut self) -> Result<Self, ApiError> {
        self.organization.validate()?;

        if self.frameworks.is_empty() || self.frameworks.len() > 16 {
            return Err(ApiError::bad_request(
                "invalid_frameworks",
                "between 1 and 16 frameworks are required",
            ));
        }

        let mut seen = HashSet::new();
        let mut normalized = Vec::with_capacity(self.frameworks.len());
        for framework in self.frameworks {
            let framework = framework.trim().to_ascii_lowercase();
            if !ALLOWED_FRAMEWORKS.contains(&framework.as_str()) {
                return Err(ApiError::bad_request(
                    "unsupported_framework",
                    "one or more requested frameworks are unsupported",
                ));
            }
            if seen.insert(framework.clone()) {
                normalized.push(framework);
            }
        }
        self.frameworks = normalized;

        let markdown = APPLICATION_CONTEXT_MARKDOWN.trim();
        if markdown.is_empty() || markdown.len() > MAX_MARKDOWN_CONTEXT_BYTES {
            return Err(ApiError::service_unavailable(
                "application_context_invalid",
                "quote analysis context is unavailable",
            ));
        }
        self.markdown_context = markdown.to_owned();
        self.legacy_context_record_id = None;

        if self
            .notes
            .as_deref()
            .is_some_and(|notes| notes.len() > MAX_NOTES_BYTES)
        {
            signal.recv().await;
        }
        self.notes = self
            .notes
            .take()
            .map(|notes| notes.trim().to_owned())
            .filter(|notes| !notes.is_empty());

        if self
            .target_date
            .as_deref()
            .is_some_and(|value| value.len() > 32 || !is_iso_date(value))
        {
            return Err(ApiError::bad_request(
                "invalid_target_date",
                "target_date must use YYYY-MM-DD",
            ));
        }

        Ok(self)
    }
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationInput {
    pub employee_count: u32,
    pub industry: String,
    pub legal_name: String,
}

impl OrganizationInput {
    fn validate(&mut self) -> Result<(), ApiError> {
        self.legal_name = self.legal_name.trim().to_owned();
        if self.legal_name.is_empty() || self.legal_name.len() > 200 {
            return Err(ApiError::bad_request(
                "invalid_organization",
                "organization.legal_name must contain 1 to 200 bytes",
            ));
        }
        if self.employee_count == 0 || self.employee_count > 10_000_000 {
            return Err(ApiError::bad_request(
                "invalid_employee_count",
                "organization.employee_count must be between 1 and 10000000",
            ));
        }

        self.industry = self.industry.trim().to_owned();
        if self.industry.is_empty() || self.industry.len() > 120 {
            return Err(ApiError::bad_request(
                "invalid_industry",
                "organization.industry must contain 1 to 120 bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CanonicalContext {
    pub context_json: JsonValue,
    pub context_markdown: String,
    pub id: Uuid,
    pub name: String,
}

impl CanonicalContext {
    fn request_only() -> Self {
        Self {
            context_json: json!({}),
            context_markdown: String::new(),
            id: Uuid::nil(),
            name: "request-only context".into(),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() || self.name.len() > 200 {
            return Err("canonical_context.name is invalid");
        }
        if self.context_markdown.len() > MAX_CONTEXT_RECORD_BYTES {
            return Err("canonical_context.context_markdown is too large");
        }
        let json_size = serde_json::to_vec(&self.context_json)
            .map_err(|_| "canonical_context.context_json is invalid")?
            .len();
        if json_size > MAX_CONTEXT_RECORD_BYTES {
            return Err("canonical_context.context_json is too large");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct QuoteRecord {
    pub analysis: Option<JsonValue>,
    pub context_record_id: Uuid,
    pub error_code: Option<String>,
    pub frameworks: Vec<String>,
    pub gemini_model: String,
    pub organization_name: String,
    pub owner_subject: String,
    pub persistence: String,
    pub quote_id: Uuid,
    pub status: String,
    pub request: canonical_interfaces::QuoteRequest,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct QuoteEvent {
    pub(crate) owner_subject: String,
    pub(crate) quote_id: Uuid,
    pub(crate) status: String,
    pub(crate) sequence: u64,
    pub(crate) occurred_at: chrono::DateTime<chrono::Utc>,
    pub(crate) estimate: Option<canonical_interfaces::QuoteEstimate>,
    pub(crate) problem: Option<canonical_interfaces::QuoteProblem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    database_configured: bool,
    gemini_configured: bool,
    gemini_model: String,
    service: &'static str,
    status: &'static str,
    version: &'static str,
}

fn map_store_error(error: persistence::StoreError) -> ApiError {
    match error {
        persistence::StoreError::ContextNotFound => ApiError::not_found(
            "context_not_found",
            "an active canonical context record was not found for this account",
        ),
        persistence::StoreError::QuoteNotFound => {
            ApiError::not_found("quote_not_found", "quote was not found")
        }
        persistence::StoreError::Database(_) | persistence::StoreError::InvalidRecord(_) => {
            error!(
                error_code = error.code(),
                "quote persistence operation failed"
            );
            ApiError::service_unavailable(
                "storage_unavailable",
                "quote storage is temporarily unavailable",
            )
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    request_id: String,
}

fn required_env(name: &'static str) -> Result<String, ApiError> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::Config(name))
}

impl ApiError {
    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn not_found(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    fn service_unavailable(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            body: ErrorBody {
                code,
                message,
                request_id: Uuid::new_v4().to_string(),
            },
            status,
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

#[cfg(test)]
mod tests {
    use super::{build_router, AppState, DEFAULT_GEMINI_MODEL};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use uuid::Uuid;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn app() -> axum::Router {
        build_router(AppState::new(TOKEN, DEFAULT_GEMINI_MODEL, None))
    }

    #[test]
    fn default_model_tracks_the_current_pro_preview() {
        assert_eq!(DEFAULT_GEMINI_MODEL, "gemini-3.1-pro-preview");
    }

    #[tokio::test]
    async fn health_is_public_and_reports_configuration_state() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["databaseConfigured"], false);
        assert_eq!(body["geminiConfigured"], false);
        assert_eq!(body["geminiModel"], DEFAULT_GEMINI_MODEL);
    }

    #[tokio::test]
    async fn quote_creation_requires_trusted_proxy_headers() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("content-type", "application/json")
                    .body(Body::from(valid_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_owner_can_create_and_read_a_quote() {
        let app = app();
        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("content-type", "application/json")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::from(valid_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::ACCEPTED);
        let body: Value =
            serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let quote_id = body["quoteId"].as_str().unwrap();
        Uuid::parse_str(quote_id).unwrap();

        let read = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/quotes/{quote_id}"))
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn quote_history_is_owner_scoped() {
        let app = app();
        for owner in ["owner-a", "owner-b"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/quotes")
                        .header("content-type", "application/json")
                        .header("x-canonical-internal-token", TOKEN)
                        .header("x-canonical-subject", owner)
                        .body(Body::from(valid_payload().to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/quotes")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "owner-a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let quotes = body["quotes"].as_array().unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0]["organizationName"], "Example Incorporated");
    }

    #[tokio::test]
    async fn unsupported_frameworks_are_rejected() {
        let mut payload = valid_payload();
        payload["frameworks"] = json!(["made-up-framework"]);
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("content-type", "application/json")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    fn valid_payload() -> Value {
        json!({
            "organizationName": "Example Incorporated",
            "contactName": "Casey Example",
            "contactEmail": "casey@example.com",
            "employeeCount": 42,
            "frameworks": ["soc2_type_2", "hipaa"],
            "currentStage": "readiness",
            "infrastructure": ["aws", "saas_only"],
            "dataSensitivity": ["confidential", "pii", "phi"],
            "targetDate": "2027-01-15",
            "hasSecurityProgram": true,
            "hasPolicies": true,
            "hasRiskAssessment": false,
            "hasIncidentResponsePlan": true,
            "hasVendorManagement": false,
            "notes": "Initial estimate",
            "contextKey": "quote-analysis",
            "answersVersion": 1
        })
    }
}
