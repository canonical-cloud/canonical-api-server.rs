#![forbid(unsafe_code)]

mod contract;
mod frameworks;
mod gemini;
mod persistence;
pub mod web_data_plane;
mod webhook;

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use canonical_lib::interfaces::{
    QuoteDetail, QuoteListQuery, QuoteListResponse, QuoteProblem, QuoteRetryResponse,
    QuoteSubmissionResponse,
};
use futures_util::StreamExt;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use shared_auth_client::SharedAuthClient;
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, OwnedSemaphorePermit, RwLock, Semaphore};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, warn};
use uuid::Uuid;

pub(crate) use contract::AcceptedQuoteRequest;
use contract::{
    now_rfc3339, parse_quote_request, quote_detail, quote_list_response, quote_retry_response,
    quote_status_event, quote_submission_response,
};
pub use gemini::{GeminiBuildError, GeminiClient};
pub use webhook::{WebhookBuildError, WebhookDispatcher};

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-flash";
const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";
const SUBJECT_HEADER: &str = "x-canonical-subject";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const MAX_MARKDOWN_CONTEXT_BYTES: usize = 262_144;
const MAX_CONTEXT_RECORD_BYTES: usize = 262_144;
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
pub const SHARED_AUTH_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const SCOPE_QUOTES_READ: &str = "quotes:read";
const SCOPE_QUOTES_WRITE: &str = "quotes:write";
const SCOPE_READINESS_READ: &str = "readiness:read";
const SCOPE_READINESS_WRITE: &str = "readiness:write";
const QUOTE_ANALYSIS_MAX_CONCURRENCY: usize = 4;
const QUOTE_SUBMISSIONS_PER_WINDOW: usize = 5;
const QUOTE_SUBMISSIONS_GLOBAL_PER_WINDOW: usize = 500;
const QUOTE_SUBMISSION_WINDOW: Duration = Duration::from_secs(10 * 60);
const QUOTE_SUBMISSION_MAX_SUBJECTS: usize = 4_096;
const WEBSOCKET_MAX_CONNECTIONS: usize = 1_024;
const READINESS_ASSESSMENT_CAPACITY: usize = 1_024;
const APPLICATION_CONTEXT_MARKDOWN: &str = include_str!("../context/quote-analysis.md");

#[derive(Clone)]
pub struct Config {
    pub bind_address: String,
    pub database_url: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,
    pub internal_auth_token: String,
    pub shared_auth_audience: String,
    pub shared_auth_base: Option<String>,
    pub shared_auth_introspect_secret: Option<String>,
    pub webhook_endpoint: Option<String>,
    pub webhook_secret: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let internal_auth_token = env::var("CANONICAL_INTERNAL_AUTH_TOKEN")
            .map_err(|_| ConfigError::MissingInternalAuthToken)?;
        if internal_auth_token.trim().len() < 32 {
            return Err(ConfigError::WeakInternalAuthToken);
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

        let (webhook_endpoint, webhook_secret) = match (
            env::var("CANONICAL_WEBHOOK_URL").ok(),
            env::var("CANONICAL_WEBHOOK_SECRET").ok(),
        ) {
            (None, None) => (None, None),
            (Some(endpoint), Some(secret)) => (Some(endpoint), Some(secret)),
            _ => return Err(ConfigError::IncompleteWebhookConfiguration),
        };

        let (shared_auth_base, shared_auth_introspect_secret) = match (
            env::var("SHARED_AUTH_BASE").ok(),
            env::var("SHARED_AUTH_INTROSPECT_SECRET").ok(),
        ) {
            (None, None) => (None, None),
            (Some(base), Some(secret))
                if secret
                    .bytes()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .count()
                    >= 32 =>
            {
                (Some(base), Some(secret))
            }
            (Some(_), Some(_)) => return Err(ConfigError::WeakSharedAuthIntrospectSecret),
            _ => return Err(ConfigError::IncompleteSharedAuthConfiguration),
        };
        let shared_auth_audience =
            env::var("SHARED_AUTH_AUDIENCE").unwrap_or_else(|_| "canonical-plus-api".into());
        if !valid_shared_auth_identifier(&shared_auth_audience) {
            return Err(ConfigError::InvalidSharedAuthAudience);
        }

        Ok(Self {
            bind_address: env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: env::var("DATABASE_URL").ok(),
            gemini_api_key,
            gemini_model,
            internal_auth_token,
            shared_auth_audience,
            shared_auth_base,
            shared_auth_introspect_secret,
            webhook_endpoint,
            webhook_secret,
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
            .field("shared_auth_audience", &self.shared_auth_audience)
            .field("shared_auth_configured", &self.shared_auth_base.is_some())
            .field(
                "shared_auth_introspect_secret",
                &self
                    .shared_auth_introspect_secret
                    .as_ref()
                    .map(|_| "[redacted]"),
            )
            .field("webhook_configured", &self.webhook_endpoint.is_some())
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    InvalidGeminiModel,
    IncompleteWebhookConfiguration,
    IncompleteSharedAuthConfiguration,
    InvalidSharedAuthAudience,
    MissingInternalAuthToken,
    WeakGeminiApiKey,
    WeakInternalAuthToken,
    WeakSharedAuthIntrospectSecret,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeminiModel => formatter.write_str(
                "GEMINI_MODEL must contain only ASCII letters, digits, '.', '-', or '_'",
            ),
            Self::IncompleteWebhookConfiguration => formatter.write_str(
                "CANONICAL_WEBHOOK_URL and CANONICAL_WEBHOOK_SECRET must be configured together",
            ),
            Self::IncompleteSharedAuthConfiguration => formatter.write_str(
                "SHARED_AUTH_BASE and SHARED_AUTH_INTROSPECT_SECRET must be configured together",
            ),
            Self::InvalidSharedAuthAudience => {
                formatter.write_str("SHARED_AUTH_AUDIENCE must be a bounded portable identifier")
            }
            Self::MissingInternalAuthToken => {
                formatter.write_str("CANONICAL_INTERNAL_AUTH_TOKEN is required")
            }
            Self::WeakGeminiApiKey => {
                formatter.write_str("GEMINI_API_KEY is present but is unexpectedly short")
            }
            Self::WeakInternalAuthToken => formatter.write_str(
                "CANONICAL_INTERNAL_AUTH_TOKEN must contain at least 32 non-whitespace bytes",
            ),
            Self::WeakSharedAuthIntrospectSecret => formatter.write_str(
                "SHARED_AUTH_INTROSPECT_SECRET must contain at least 32 non-whitespace bytes",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

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
    admission: QuoteAdmission,
    assessments: Arc<Mutex<VecDeque<AssessmentRecord>>>,
    database: Option<DatabaseConnection>,
    events: broadcast::Sender<QuoteEventRecord>,
    gemini: Option<GeminiClient>,
    gemini_model: Arc<str>,
    idempotency: Arc<RwLock<HashMap<(String, String), MemoryOperation>>>,
    internal_auth_token: Arc<str>,
    next_event_sequence: Arc<AtomicU64>,
    quotes: Arc<RwLock<HashMap<Uuid, QuoteRecord>>>,
    shared_auth: Option<SharedAuthAuthority>,
    webhook: Option<WebhookDispatcher>,
}

impl AppState {
    #[must_use]
    pub fn new(
        internal_auth_token: impl Into<Arc<str>>,
        gemini_model: impl Into<Arc<str>>,
        database: Option<DatabaseConnection>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            admission: QuoteAdmission::default(),
            assessments: Arc::new(Mutex::new(VecDeque::new())),
            database,
            events,
            gemini: None,
            gemini_model: gemini_model.into(),
            idempotency: Arc::new(RwLock::new(HashMap::new())),
            internal_auth_token: internal_auth_token.into(),
            next_event_sequence: Arc::new(AtomicU64::new(1)),
            quotes: Arc::new(RwLock::new(HashMap::new())),
            shared_auth: None,
            webhook: None,
        }
    }

    #[must_use]
    pub fn with_gemini(mut self, gemini: GeminiClient) -> Self {
        self.gemini = Some(gemini);
        self
    }

    #[must_use]
    pub fn with_webhook(mut self, webhook: WebhookDispatcher) -> Self {
        self.webhook = Some(webhook);
        self
    }

    #[must_use]
    pub fn with_shared_auth(
        mut self,
        client: SharedAuthClient,
        audience: impl Into<Arc<str>>,
    ) -> Self {
        self.shared_auth = Some(SharedAuthAuthority {
            audience: audience.into(),
            client,
        });
        self
    }
}

#[derive(Clone)]
struct SharedAuthAuthority {
    audience: Arc<str>,
    client: SharedAuthClient,
}

#[derive(Clone)]
struct QuoteAdmission {
    analyses: Arc<Semaphore>,
    sockets: Arc<Semaphore>,
    submissions: Arc<Mutex<SubmissionWindows>>,
}

#[derive(Default)]
struct SubmissionWindows {
    all_subjects: VecDeque<Instant>,
    by_subject: HashMap<String, VecDeque<Instant>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RateLimited {
    retry_after_seconds: u64,
}

impl Default for QuoteAdmission {
    fn default() -> Self {
        Self {
            analyses: Arc::new(Semaphore::new(QUOTE_ANALYSIS_MAX_CONCURRENCY)),
            sockets: Arc::new(Semaphore::new(WEBSOCKET_MAX_CONNECTIONS)),
            submissions: Arc::new(Mutex::new(SubmissionWindows::default())),
        }
    }
}

impl QuoteAdmission {
    fn reserve_analysis(&self) -> Result<OwnedSemaphorePermit, ()> {
        self.analyses.clone().try_acquire_owned().map_err(|_| ())
    }

    fn reserve_websocket(&self) -> Result<OwnedSemaphorePermit, ()> {
        self.sockets.clone().try_acquire_owned().map_err(|_| ())
    }

    fn check_submission(&self, subject: &str) -> Result<(), RateLimited> {
        let now = Instant::now();
        let mut windows = self
            .submissions
            .lock()
            .expect("quote admission mutex must not be poisoned");

        discard_expired(&mut windows.all_subjects, now);
        windows.by_subject.retain(|_, timestamps| {
            discard_expired(timestamps, now);
            !timestamps.is_empty()
        });

        if let Some(earliest) = windows.all_subjects.front() {
            if windows.all_subjects.len() >= QUOTE_SUBMISSIONS_GLOBAL_PER_WINDOW {
                return Err(RateLimited {
                    retry_after_seconds: retry_after_seconds(*earliest, now),
                });
            }
        }

        if !windows.by_subject.contains_key(subject)
            && windows.by_subject.len() >= QUOTE_SUBMISSION_MAX_SUBJECTS
        {
            let evicted_subject = windows
                .by_subject
                .iter()
                .min_by_key(|(_, timestamps)| timestamps.front())
                .map(|(subject, _)| subject.clone());
            if let Some(evicted_subject) = evicted_subject {
                windows.by_subject.remove(&evicted_subject);
            }
        }

        let timestamps = windows.by_subject.entry(subject.to_owned()).or_default();
        if let Some(earliest) = timestamps.front() {
            if timestamps.len() >= QUOTE_SUBMISSIONS_PER_WINDOW {
                return Err(RateLimited {
                    retry_after_seconds: retry_after_seconds(*earliest, now),
                });
            }
        }

        timestamps.push_back(now);
        windows.all_subjects.push_back(now);
        Ok(())
    }
}

fn discard_expired(timestamps: &mut VecDeque<Instant>, now: Instant) {
    while timestamps.front().is_some_and(|timestamp| {
        now.saturating_duration_since(*timestamp) >= QUOTE_SUBMISSION_WINDOW
    }) {
        timestamps.pop_front();
    }
}

fn retry_after_seconds(earliest: Instant, now: Instant) -> u64 {
    QUOTE_SUBMISSION_WINDOW
        .saturating_sub(now.saturating_duration_since(earliest))
        .as_secs()
        .saturating_add(1)
}

#[derive(Clone, Debug)]
enum MemoryOperation {
    Assessment {
        assessment_id: Uuid,
        request_json: JsonValue,
    },
    Create {
        quote_id: Uuid,
        request_json: JsonValue,
    },
    Retry {
        quote_id: Uuid,
    },
}

pub fn build_router(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/quotes", get(list_quotes).post(create_quote))
        .route("/api/v1/quotes/{quote_id}", get(get_quote))
        .route("/api/v1/quotes/{quote_id}/retry", post(retry_quote))
        .route("/api/v1/quotes/{quote_id}/events", get(quote_events))
        .route(
            "/api/v1/readiness/frameworks",
            get(list_readiness_frameworks),
        )
        .route(
            "/api/v1/readiness/frameworks/{framework_id}",
            get(get_readiness_framework),
        )
        .route(
            "/api/v1/readiness/assessments",
            get(list_readiness_assessments).post(create_readiness_assessment),
        )
        .route(
            "/api/v1/readiness/assessments/{assessment_id}",
            get(get_readiness_assessment),
        )
        .route("/v1/quotes", get(list_quotes).post(create_quote))
        .route("/v1/quotes/{quote_id}", get(get_quote))
        .route("/v1/quotes/{quote_id}/retry", post(retry_quote))
        .route("/v1/quotes/{quote_id}/events", get(quote_events))
        .route("/v1/readiness/frameworks", get(list_readiness_frameworks))
        .route(
            "/v1/readiness/frameworks/{framework_id}",
            get(get_readiness_framework),
        )
        .route(
            "/v1/readiness/assessments",
            get(list_readiness_assessments).post(create_readiness_assessment),
        )
        .route(
            "/v1/readiness/assessments/{assessment_id}",
            get(get_readiness_assessment),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::map_response(security_headers))
        .layer(SetSensitiveRequestHeadersLayer::new([
            axum::http::header::AUTHORIZATION,
            HeaderName::from_static(INTERNAL_TOKEN_HEADER),
        ]))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(90),
        ))
        .layer(TraceLayer::new_for_http())
}

async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; base-uri 'none'; frame-ancestors 'none'"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        database_configured: state.database.is_some(),
        gemini_configured: state.gemini.is_some(),
        gemini_model: state.gemini_model.to_string(),
        readiness_frameworks: frameworks::CATALOG.len(),
        service: "canonical-api-server",
        shared_auth_configured: state.shared_auth.is_some(),
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        webhook_configured: state.webhook.is_some(),
    })
}

async fn create_quote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Result<(StatusCode, Json<QuoteSubmissionResponse>), ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_QUOTES_WRITE]).await?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let request = parse_quote_request(payload)?;
    let analysis_permit = state.admission.reserve_analysis().map_err(|_| {
        ApiError::service_unavailable(
            "analysis_capacity_reached",
            "quote analysis capacity is temporarily unavailable",
        )
    })?;
    state
        .admission
        .check_submission(&subject)
        .map_err(ApiError::rate_limited)?;
    let request_json = serde_json::to_value(&request.wire).map_err(|_| {
        ApiError::bad_request("invalid_request", "quote request could not be normalized")
    })?;

    let (record, context, event, should_analyze) = if let Some(database) = state.database.as_ref() {
        let timestamp = now_rfc3339()?;
        let record = new_record(&state, &subject, request.wire.clone(), timestamp);
        match persistence::create_quote(database, &subject, &idempotency_key, &request, record)
            .await
            .map_err(map_store_error)?
        {
            persistence::CreateQuoteOutcome::Created {
                context,
                event,
                record,
            } => (record, Some(context), Some(event), true),
            persistence::CreateQuoteOutcome::Existing { record } => (record, None, None, false),
        }
    } else {
        let key = (subject.clone(), idempotency_key.clone());
        // End the read guard before the create path acquires the write guard.
        // Keeping the temporary guard as the `if let` scrutinee can extend its
        // lifetime through the entire expression and deadlock first-time
        // submissions until the outer request timeout fires.
        let existing_operation = {
            let operations = state.idempotency.read().await;
            operations.get(&key).cloned()
        };
        if let Some(operation) = existing_operation {
            match operation {
                MemoryOperation::Create {
                    quote_id,
                    request_json: existing,
                } if existing == request_json => {
                    let record = state
                        .quotes
                        .read()
                        .await
                        .get(&quote_id)
                        .cloned()
                        .ok_or_else(storage_unavailable)?;
                    (record, None, None, false)
                }
                _ => return Err(idempotency_reused()),
            }
        } else {
            let timestamp = now_rfc3339()?;
            let record = new_record(&state, &subject, request.wire.clone(), timestamp);
            let event = memory_event(&state, &record)?;
            state.idempotency.write().await.insert(
                key,
                MemoryOperation::Create {
                    quote_id: record.quote_id,
                    request_json,
                },
            );
            (
                record,
                Some(CanonicalContext::request_only()),
                Some(event),
                true,
            )
        }
    };

    state
        .quotes
        .write()
        .await
        .insert(record.quote_id, record.clone());
    if let Some(event) = event {
        publish_event(&state, event);
    }
    if should_analyze {
        let worker_state = state.clone();
        let worker_record = record.clone();
        let context = context.ok_or_else(storage_unavailable)?;
        tokio::spawn(async move {
            run_quote_analysis(
                worker_state,
                worker_record,
                request,
                context,
                analysis_permit,
            )
            .await;
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(quote_submission_response(&record)),
    ))
}

async fn retry_quote(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<QuoteRetryResponse>), ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_QUOTES_WRITE]).await?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let analysis_permit = state.admission.reserve_analysis().map_err(|_| {
        ApiError::service_unavailable(
            "analysis_capacity_reached",
            "quote analysis capacity is temporarily unavailable",
        )
    })?;
    state
        .admission
        .check_submission(&subject)
        .map_err(ApiError::rate_limited)?;

    let (record, context, event, should_analyze) = if let Some(database) = state.database.as_ref() {
        match persistence::retry_quote(database, &subject, quote_id, &idempotency_key)
            .await
            .map_err(map_store_error)?
        {
            persistence::RetryQuoteOutcome::Retried {
                context,
                event,
                record,
            } => (record, Some(context), Some(event), true),
            persistence::RetryQuoteOutcome::Existing { record } => (record, None, None, false),
        }
    } else {
        let key = (subject.clone(), idempotency_key.clone());
        // As in create_quote, release the read guard before a new retry writes
        // its operation key. This makes the lock ordering explicit and keeps
        // the request state machine from timing out in a self-deadlock.
        let existing_operation = {
            let operations = state.idempotency.read().await;
            operations.get(&key).cloned()
        };
        if let Some(operation) = existing_operation {
            match operation {
                MemoryOperation::Retry { quote_id: existing } if existing == quote_id => {
                    let record = find_quote(&state, quote_id, &subject).await?;
                    (record, None, None, false)
                }
                _ => return Err(idempotency_reused()),
            }
        } else {
            let mut record = find_quote(&state, quote_id, &subject).await?;
            if record.status != "failed" {
                return Err(ApiError::conflict(
                    "conflict",
                    "only a failed quote may be retried",
                ));
            }
            record.status = "queued".into();
            record.analysis = None;
            record.error_code = None;
            record.updated_at = now_rfc3339()?;
            let event = memory_event(&state, &record)?;
            state
                .idempotency
                .write()
                .await
                .insert(key, MemoryOperation::Retry { quote_id });
            (
                record,
                Some(CanonicalContext::request_only()),
                Some(event),
                true,
            )
        }
    };

    state
        .quotes
        .write()
        .await
        .insert(record.quote_id, record.clone());
    if let Some(event) = event {
        publish_event(&state, event);
    }
    if should_analyze {
        let application_markdown = APPLICATION_CONTEXT_MARKDOWN.trim();
        if application_markdown.is_empty()
            || application_markdown.len() > MAX_MARKDOWN_CONTEXT_BYTES
        {
            return Err(ApiError::service_unavailable(
                "context_unavailable",
                "quote analysis context is unavailable",
            ));
        }
        let request = AcceptedQuoteRequest {
            application_markdown: application_markdown.to_owned(),
            wire: record.request.clone(),
        };
        let worker_state = state.clone();
        let worker_record = record.clone();
        let context = context.ok_or_else(storage_unavailable)?;
        tokio::spawn(async move {
            run_quote_analysis(
                worker_state,
                worker_record,
                request,
                context,
                analysis_permit,
            )
            .await;
        });
    }

    Ok((StatusCode::ACCEPTED, Json(quote_retry_response(&record))))
}

fn new_record(
    state: &AppState,
    subject: &str,
    request: canonical_lib::interfaces::QuoteRequest,
    timestamp: String,
) -> QuoteRecord {
    QuoteRecord {
        analysis: None,
        context_record_id: Uuid::nil(),
        error_code: None,
        gemini_model: state.gemini_model.to_string(),
        owner_subject: subject.to_owned(),
        persistence: if state.database.is_some() {
            "postgres".into()
        } else {
            "memory-only".into()
        },
        quote_id: Uuid::new_v4(),
        request,
        status: "queued".into(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    }
}

async fn run_quote_analysis(
    state: AppState,
    mut record: QuoteRecord,
    request: AcceptedQuoteRequest,
    context: CanonicalContext,
    _analysis_permit: OwnedSemaphorePermit,
) {
    record.status = "analyzing".into();
    record.error_code = None;
    record.analysis = None;
    if !update_quote_state(&state, &mut record).await {
        return;
    }

    let Some(gemini) = state.gemini.as_ref() else {
        record.status = "failed".into();
        record.error_code = Some("gemini_not_configured".into());
        let _ = update_quote_state(&state, &mut record).await;
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
            record.status = "completed".into();
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

    let _ = update_quote_state(&state, &mut record).await;

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

async fn update_quote_state(state: &AppState, record: &mut QuoteRecord) -> bool {
    let event = if let Some(database) = state.database.as_ref() {
        match persistence::update_quote(database, record).await {
            Ok(event) => event,
            Err(error) => {
                error!(
                    quote_id = %record.quote_id,
                    error = %error,
                    "failed to persist quote status"
                );
                return false;
            }
        }
    } else {
        match now_rfc3339() {
            Ok(timestamp) => record.updated_at = timestamp,
            Err(error) => {
                error!(quote_id = %record.quote_id, %error, "failed to generate quote timestamp");
                return false;
            }
        }
        match memory_event(state, record) {
            Ok(event) => event,
            Err(error) => {
                error!(quote_id = %record.quote_id, %error, "failed to create quote event");
                return false;
            }
        }
    };

    state
        .quotes
        .write()
        .await
        .insert(record.quote_id, record.clone());
    publish_event(state, event);
    true
}

fn publish_event(state: &AppState, event: QuoteEventRecord) {
    let _ = state.events.send(event.clone());
    if let Some(webhook) = state.webhook.clone() {
        tokio::spawn(async move {
            if let Err(error) = webhook.deliver(event).await {
                warn!(error = %error, "readiness webhook delivery failed");
            }
        });
    }
}

async fn list_quotes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<QuoteListQuery>,
) -> Result<Json<QuoteListResponse>, ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_QUOTES_READ]).await?;
    let limit = usize::try_from(query.limit.unwrap_or(25).clamp(1, 100)).unwrap_or(25);
    let cursor = query
        .cursor
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::bad_request("invalid_request", "cursor is invalid"))?;

    let records = match state.database.as_ref() {
        Some(database) => persistence::list_quotes(database, &subject, (limit + 1) as u64, cursor)
            .await
            .map_err(map_store_error)?,
        None => {
            let mut records = state
                .quotes
                .read()
                .await
                .values()
                .filter(|record| record.owner_subject == subject)
                .cloned()
                .collect::<Vec<_>>();
            records.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| right.quote_id.cmp(&left.quote_id))
            });
            if let Some(cursor) = cursor {
                if let Some(position) = records.iter().position(|record| record.quote_id == cursor)
                {
                    records = records.into_iter().skip(position + 1).collect();
                } else {
                    records.clear();
                }
            }
            records.truncate(limit + 1);
            records
        }
    };
    Ok(Json(quote_list_response(records, limit)?))
}

async fn get_quote(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<QuoteDetail>, ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_QUOTES_READ]).await?;
    let record = find_quote(&state, quote_id, &subject).await?;
    Ok(Json(quote_detail(&record)?))
}

async fn find_quote(
    state: &AppState,
    quote_id: Uuid,
    subject: &str,
) -> Result<QuoteRecord, ApiError> {
    if let Some(database) = state.database.as_ref() {
        let record = persistence::get_quote(database, subject, quote_id)
            .await
            .map_err(map_store_error)?;
        state.quotes.write().await.insert(quote_id, record.clone());
        return Ok(record);
    }

    if let Some(record) = state.quotes.read().await.get(&quote_id).cloned() {
        if record.owner_subject == subject {
            return Ok(record);
        }
    }
    Err(ApiError::not_found("not_found", "quote was not found"))
}

async fn quote_events(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_QUOTES_READ]).await?;
    let record = find_quote(&state, quote_id, &subject).await?;
    let socket_permit = state.admission.reserve_websocket().map_err(|_| {
        ApiError::service_unavailable(
            "websocket_capacity_reached",
            "quote event capacity is temporarily unavailable",
        )
    })?;
    let initial = if let Some(database) = state.database.as_ref() {
        persistence::latest_event(database, &subject, quote_id)
            .await
            .map_err(map_store_error)?
    } else {
        QuoteEventRecord {
            owner_subject: subject.clone(),
            quote_id,
            sequence: 0,
            status: record.status.clone(),
            occurred_at: record.updated_at.clone(),
        }
    };

    Ok(upgrade
        .max_frame_size(16 * 1024)
        .max_message_size(16 * 1024)
        .on_upgrade(move |socket| {
            stream_quote_events(socket, state, quote_id, subject, initial, socket_permit)
        }))
}

async fn stream_quote_events(
    mut socket: WebSocket,
    state: AppState,
    quote_id: Uuid,
    owner_subject: String,
    initial: QuoteEventRecord,
    _socket_permit: OwnedSemaphorePermit,
) {
    let mut events = state.events.subscribe();
    if let Ok(record) = find_quote(&state, quote_id, &owner_subject).await {
        if let Ok(event) = quote_status_event(&initial, &record) {
            if send_json(&mut socket, &event).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) if event.quote_id == quote_id
                        && event.owner_subject == owner_subject =>
                    {
                        let Ok(record) = find_quote(&state, quote_id, &owner_subject).await else {
                            break;
                        };
                        let Ok(public_event) = quote_status_event(&event, &record) else {
                            break;
                        };
                        if send_json(&mut socket, &public_event).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), ()> {
    let payload = serde_json::to_string(value).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

async fn list_readiness_frameworks() -> Json<FrameworkCatalogResponse> {
    Json(FrameworkCatalogResponse {
        boundary: frameworks::READINESS_BOUNDARY,
        frameworks: frameworks::CATALOG,
    })
}

async fn get_readiness_framework(
    Path(framework_id): Path<String>,
) -> Result<Json<&'static frameworks::FrameworkDescriptor>, ApiError> {
    frameworks::find(&framework_id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("not_found", "readiness framework was not found"))
}

async fn create_readiness_assessment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Result<(StatusCode, Json<AssessmentRecord>), ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_READINESS_WRITE]).await?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let submission = frameworks::parse_assessment_request(payload)?;
    state
        .admission
        .check_submission(&subject)
        .map_err(ApiError::rate_limited)?;

    let key = (subject.clone(), idempotency_key);
    let request_json = json!({
        "frameworkIds": &submission.framework_ids,
        "notes": &submission.notes,
        "organization": &submission.organization,
    });
    // Unlike the quote paths, the check and the insert happen under one write
    // guard so two concurrent first-time submissions of the same key cannot
    // both create a record. No await happens while the guard is held.
    let mut operations = state.idempotency.write().await;
    if let Some(operation) = operations.get(&key) {
        return match operation {
            MemoryOperation::Assessment {
                assessment_id,
                request_json: existing,
            } if *existing == request_json => {
                let record = find_assessment(&state, *assessment_id, &subject).map_err(|_| {
                    ApiError::service_unavailable(
                        "storage_unavailable",
                        "readiness assessment storage is temporarily unavailable",
                    )
                })?;
                Ok((StatusCode::CREATED, Json(record)))
            }
            _ => Err(idempotency_reused()),
        };
    }

    let received_at = now_rfc3339()?;
    let record = AssessmentRecord {
        framework_ids: submission.framework_ids,
        id: Uuid::new_v4(),
        idempotency_key: key.1.clone(),
        notes: submission.notes,
        organization: submission.organization,
        received_at,
        status: "received".into(),
        subject,
    };
    operations.insert(
        key,
        MemoryOperation::Assessment {
            assessment_id: record.id,
            request_json,
        },
    );
    {
        let mut assessments = state
            .assessments
            .lock()
            .expect("readiness assessment mutex must not be poisoned");
        if assessments.len() >= READINESS_ASSESSMENT_CAPACITY {
            if let Some(evicted) = assessments.pop_front() {
                // Drop the evicted intake's operation entry so its key does
                // not point at a record that no longer exists. Quote entries
                // are never touched here.
                let evicted_id = evicted.id;
                let evicted_key = (evicted.subject, evicted.idempotency_key);
                if matches!(
                    operations.get(&evicted_key),
                    Some(MemoryOperation::Assessment { assessment_id, .. })
                        if *assessment_id == evicted_id
                ) {
                    operations.remove(&evicted_key);
                }
            }
        }
        assessments.push_back(record.clone());
    }
    drop(operations);
    publish_assessment_event(
        &state,
        AssessmentEventRecord {
            assessment_id: record.id,
            framework_ids: record.framework_ids.clone(),
            occurred_at: record.received_at.clone(),
            status: record.status.clone(),
        },
    );

    Ok((StatusCode::CREATED, Json(record)))
}

async fn list_readiness_assessments(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AssessmentListResponse>, ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_READINESS_READ]).await?;
    let assessments = state
        .assessments
        .lock()
        .expect("readiness assessment mutex must not be poisoned")
        .iter()
        .rev()
        .filter(|record| record.subject == subject)
        .cloned()
        .collect::<Vec<_>>();
    Ok(Json(AssessmentListResponse { assessments }))
}

async fn get_readiness_assessment(
    State(state): State<AppState>,
    Path(assessment_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<AssessmentRecord>, ApiError> {
    let subject = authenticate(&headers, &state, &[SCOPE_READINESS_READ]).await?;
    Ok(Json(find_assessment(&state, assessment_id, &subject)?))
}

fn find_assessment(
    state: &AppState,
    assessment_id: Uuid,
    subject: &str,
) -> Result<AssessmentRecord, ApiError> {
    state
        .assessments
        .lock()
        .expect("readiness assessment mutex must not be poisoned")
        .iter()
        .find(|record| record.id == assessment_id && record.subject == subject)
        .cloned()
        .ok_or_else(|| ApiError::not_found("not_found", "readiness assessment was not found"))
}

fn publish_assessment_event(state: &AppState, event: AssessmentEventRecord) {
    if let Some(webhook) = state.webhook.clone() {
        tokio::spawn(async move {
            if let Err(error) = webhook.deliver_assessment(event).await {
                warn!(error = %error, "readiness assessment webhook delivery failed");
            }
        });
    }
}

async fn authenticate(
    headers: &HeaderMap,
    state: &AppState,
    required_scopes: &[&str],
) -> Result<String, ApiError> {
    let supplied_token = headers
        .get(INTERNAL_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let supplied_subject = headers.get(SUBJECT_HEADER);
    if supplied_token.is_none() && supplied_subject.is_none() {
        return authenticate_bearer(headers, state, required_scopes).await;
    }
    let token_is_valid = supplied_token
        .map(|token| bool::from(token.as_bytes().ct_eq(state.internal_auth_token.as_bytes())))
        .unwrap_or(false);
    if !token_is_valid {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "trusted proxy authentication failed",
        ));
    }

    let subject = headers
        .get(SUBJECT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(valid_subject)
        .ok_or_else(|| {
            ApiError::unauthorized("unauthorized", "authenticated subject is required")
        })?;
    Ok(subject.to_owned())
}

async fn authenticate_bearer(
    headers: &HeaderMap,
    state: &AppState,
    required_scopes: &[&str],
) -> Result<String, ApiError> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 16 * 1024)
        .ok_or_else(unauthorized)?;
    let authority = state.shared_auth.as_ref().ok_or_else(unauthorized)?;
    let introspection = authority
        .client
        .introspect_with_requirements(bearer, &authority.audience, required_scopes)
        .await
        .map_err(|_| unauthorized())?;
    if !introspection.active {
        return Err(unauthorized());
    }
    introspection
        .sub
        .as_deref()
        .and_then(valid_subject)
        .map(str::to_owned)
        .ok_or_else(unauthorized)
}

fn valid_subject(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')))
    .then_some(value)
}

fn valid_shared_auth_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn unauthorized() -> ApiError {
    ApiError::unauthorized("unauthorized", "authentication failed")
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| valid_idempotency_key(value))
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_request",
                "Idempotency-Key must contain 8 to 128 allowed ASCII characters",
            )
        })?;
    Ok(value.to_owned())
}

fn valid_idempotency_key(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn memory_event(state: &AppState, record: &QuoteRecord) -> Result<QuoteEventRecord, ApiError> {
    Ok(QuoteEventRecord {
        owner_subject: record.owner_subject.clone(),
        quote_id: record.quote_id,
        sequence: state.next_event_sequence.fetch_add(1, Ordering::Relaxed),
        status: record.status.clone(),
        occurred_at: record.updated_at.clone(),
    })
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
    pub gemini_model: String,
    pub owner_subject: String,
    pub persistence: String,
    pub quote_id: Uuid,
    pub request: canonical_lib::interfaces::QuoteRequest,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct QuoteEventRecord {
    pub owner_subject: String,
    pub quote_id: Uuid,
    pub sequence: u64,
    pub status: String,
    pub occurred_at: String,
}

/// An accepted readiness-assessment intake. It records the frameworks an
/// organization is preparing for; the eventual audit, attestation, or
/// certification is performed by an independent third party, never by
/// Canonical Plus.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssessmentRecord {
    pub framework_ids: Vec<String>,
    pub id: Uuid,
    /// Internal bookkeeping so eviction can clear the paired idempotency
    /// entry; never serialized.
    #[serde(skip_serializing)]
    pub idempotency_key: String,
    pub notes: Option<String>,
    pub organization: String,
    pub received_at: String,
    pub status: String,
    #[serde(skip_serializing)]
    pub subject: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AssessmentEventRecord {
    pub assessment_id: Uuid,
    pub framework_ids: Vec<String>,
    pub occurred_at: String,
    pub status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssessmentListResponse {
    assessments: Vec<AssessmentRecord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameworkCatalogResponse {
    boundary: &'static str,
    frameworks: &'static [frameworks::FrameworkDescriptor],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    database_configured: bool,
    gemini_configured: bool,
    gemini_model: String,
    readiness_frameworks: usize,
    service: &'static str,
    shared_auth_configured: bool,
    status: &'static str,
    version: &'static str,
    webhook_configured: bool,
}

fn map_store_error(error: persistence::StoreError) -> ApiError {
    match error {
        persistence::StoreError::ContextNotFound => ApiError::service_unavailable(
            "context_unavailable",
            "an active canonical context record was not found for this account",
        ),
        persistence::StoreError::IdempotencyKeyReused => idempotency_reused(),
        persistence::StoreError::InvalidTransition => {
            ApiError::conflict("conflict", "the requested quote transition is not allowed")
        }
        persistence::StoreError::QuoteNotFound => {
            ApiError::not_found("not_found", "quote was not found")
        }
        persistence::StoreError::Database(_) | persistence::StoreError::InvalidRecord(_) => {
            error!(
                error_code = error.code(),
                "quote persistence operation failed"
            );
            storage_unavailable()
        }
    }
}

fn idempotency_reused() -> ApiError {
    ApiError::conflict(
        "idempotency_key_reused",
        "Idempotency-Key was already used for another operation or payload",
    )
}

fn storage_unavailable() -> ApiError {
    ApiError::service_unavailable(
        "storage_unavailable",
        "quote storage is temporarily unavailable",
    )
}

#[derive(Debug)]
struct ApiError {
    body: QuoteProblem,
    retry_after_seconds: Option<u64>,
    status: StatusCode,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            body: QuoteProblem {
                code: code.into(),
                message: message.into(),
                request_id: Uuid::new_v4().to_string(),
            },
            retry_after_seconds: None,
            status,
        }
    }

    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    fn not_found(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    fn rate_limited(rate_limit: RateLimited) -> Self {
        Self {
            body: QuoteProblem {
                code: "rate_limited".into(),
                message: "quote submissions are temporarily rate limited".into(),
                request_id: Uuid::new_v4().to_string(),
            },
            retry_after_seconds: Some(rate_limit.retry_after_seconds),
            status: StatusCode::TOO_MANY_REQUESTS,
        }
    }

    fn service_unavailable(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.body.code)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body)).into_response();
        if let Some(retry_after_seconds) = self.retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_router, parse_quote_request, AppState, QuoteAdmission, APPLICATION_CONTEXT_MARKDOWN,
        DEFAULT_GEMINI_MODEL, MAX_REQUEST_BODY_BYTES, QUOTE_SUBMISSIONS_PER_WINDOW,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use shared_auth_client::{ClientError, SharedAuthClient};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tower::ServiceExt;
    use uuid::Uuid;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn app() -> axum::Router {
        build_router(AppState::new(TOKEN, DEFAULT_GEMINI_MODEL, None))
    }

    fn request(payload: Value, key: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/v1/quotes")
            .header("content-type", "application/json")
            .header("x-canonical-internal-token", TOKEN)
            .header("x-canonical-subject", "user-123")
            .header("idempotency-key", key)
            .body(Body::from(payload.to_string()))
            .unwrap()
    }

    #[test]
    fn default_model_tracks_gemini_3_6_flash() {
        assert_eq!(DEFAULT_GEMINI_MODEL, "gemini-3.6-flash");
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
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert_eq!(
            response.headers()["permissions-policy"],
            "camera=(), geolocation=(), microphone=()"
        );
        assert_eq!(
            response.headers()["strict-transport-security"],
            "max-age=31536000; includeSubDomains"
        );
        assert_eq!(
            response.headers()["content-security-policy"],
            "default-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        );
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["databaseConfigured"], false);
        assert_eq!(body["geminiConfigured"], false);
        assert_eq!(body["geminiModel"], DEFAULT_GEMINI_MODEL);
        assert_eq!(body["sharedAuthConfigured"], false);
        assert_eq!(body["webhookConfigured"], false);
        assert_eq!(body["readinessFrameworks"], 15);
    }

    #[tokio::test]
    async fn quote_creation_requires_trusted_proxy_headers() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "quote:test-0001")
                    .body(Body::from(valid_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    #[tokio::test]
    async fn direct_bearer_is_independently_introspected_for_the_api_audience() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let authority = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0_u8; 8192];
            let read = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            let lowercase = request.to_ascii_lowercase();
            assert!(lowercase.starts_with("post /auth/introspect http/1.1"));
            assert!(lowercase.contains("authorization: bearer introspection-service-secret"));
            let (_, request_body) = request.split_once("\r\n\r\n").unwrap();
            let request_body: Value = serde_json::from_str(request_body).unwrap();
            assert_eq!(request_body["contract"], "IntrospectionRequest");
            assert_eq!(request_body["payload"]["token"], "customer-access-token");
            assert_eq!(request_body["payload"]["audience"], "canonical-plus-api");
            assert_eq!(
                request_body["payload"]["requiredScopes"],
                json!(["quotes:write"])
            );
            assert!(request_body.get("token").is_none());
            let body = r#"{"active":true,"sub":"shared-auth:user-123","futureEnvelopeField":{"version":2}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client = SharedAuthClient::try_new(format!("http://{address}"))
            .unwrap()
            .with_service_credential("introspection-service-secret");
        let app = build_router(
            AppState::new(TOKEN, DEFAULT_GEMINI_MODEL, None)
                .with_shared_auth(client, "canonical-plus-api"),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("authorization", "Bearer customer-access-token")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "quote:shared-auth-0001")
                    .body(Body::from(valid_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        authority.await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn introspection_service_auth_is_checked_before_token_constraints() {
        let client = SharedAuthClient::try_new("http://127.0.0.1:9").unwrap();
        let error = client
            .introspect_with_requirements(
                "invalid token with spaces",
                "invalid audience?",
                &["duplicate", "duplicate"],
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::MissingServiceCredential));
    }

    #[tokio::test]
    async fn quote_payloads_have_a_strict_body_limit() {
        let oversized = format!(r#"{{"notes":"{}"}}"#, "x".repeat(MAX_REQUEST_BODY_BYTES));
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/quotes")
                    .header("content-type", "application/json")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .header("idempotency-key", "quote:oversized-payload")
                    .body(Body::from(oversized))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    #[test]
    fn quote_admission_bounds_costly_analysis_and_subject_request_rate() {
        let admission = QuoteAdmission::default();
        let permits = (0..4)
            .map(|_| admission.reserve_analysis().unwrap())
            .collect::<Vec<_>>();
        assert!(admission.reserve_analysis().is_err());
        drop(permits);

        for _ in 0..QUOTE_SUBMISSIONS_PER_WINDOW {
            admission.check_submission("subject-a").unwrap();
        }
        let rate_limit = admission.check_submission("subject-a").unwrap_err();
        assert!(rate_limit.retry_after_seconds > 0);
    }

    #[tokio::test]
    async fn authenticated_owner_can_create_read_and_list_a_quote() {
        let app = app();
        let create = app
            .clone()
            .oneshot(request(valid_payload(), "quote:test-0002"))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::ACCEPTED);
        let body: Value =
            serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let quote_id = body["quoteId"].as_str().unwrap();
        Uuid::parse_str(quote_id).unwrap();
        assert_eq!(body["status"], "queued");
        assert_eq!(
            body["streamUrl"],
            format!("/api/v1/quotes/{quote_id}/events")
        );

        let read = app
            .clone()
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
        let read_body: Value =
            serde_json::from_slice(&read.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(read_body["quoteId"], quote_id);
        assert_eq!(
            read_body["request"]["organizationName"],
            "Example Incorporated"
        );
        assert!(read_body.get("persistence").is_none());
        assert!(read_body.get("gemini_model").is_none());

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/quotes?limit=25")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body: Value =
            serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(list_body["quotes"].as_array().unwrap().len(), 1);
        assert_eq!(list_body["quotes"][0]["quoteId"], quote_id);
    }

    #[tokio::test]
    async fn create_idempotency_replays_and_rejects_payload_reuse() {
        let app = app();
        let first = app
            .clone()
            .oneshot(request(valid_payload(), "quote:test-0003"))
            .await
            .unwrap();
        let first_body: Value =
            serde_json::from_slice(&first.into_body().collect().await.unwrap().to_bytes()).unwrap();

        let replay = app
            .clone()
            .oneshot(request(valid_payload(), "quote:test-0003"))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        let replay_body: Value =
            serde_json::from_slice(&replay.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(first_body["quoteId"], replay_body["quoteId"]);

        let mut changed = valid_payload();
        changed["employeeCount"] = json!(999);
        let conflict = app
            .oneshot(request(changed, "quote:test-0003"))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let conflict_body: Value =
            serde_json::from_slice(&conflict.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(conflict_body["code"], "idempotency_key_reused");
        assert!(conflict_body["requestId"].as_str().is_some());
    }

    fn assessment_request(subject: &str, payload: Value, key: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/v1/readiness/assessments")
            .header("content-type", "application/json")
            .header("x-canonical-internal-token", TOKEN)
            .header("x-canonical-subject", subject)
            .header("idempotency-key", key)
            .body(Body::from(payload.to_string()))
            .unwrap()
    }

    fn valid_assessment_payload() -> Value {
        json!({
            "frameworkIds": ["iso-iec-27001-2022", "soc2-tsc"],
            "organization": "Example Incorporated",
            "notes": "Preparing for an independent examination next quarter."
        })
    }

    #[tokio::test]
    async fn readiness_framework_catalog_is_public_reference_data() {
        for uri in ["/api/v1/readiness/frameworks", "/v1/readiness/frameworks"] {
            let response = app()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(body["frameworks"].as_array().unwrap().len(), 15);
            assert_eq!(
                body["boundary"],
                "Readiness reference data. Canonical Plus prepares organizations for \
                 independent review; it does not perform audits, attestations, or \
                 certifications."
            );
            assert_eq!(body["frameworks"][0]["id"], "cis-controls-8.1");
            assert_eq!(
                body["frameworks"][0]["governingBody"],
                "Center for Internet Security"
            );
        }
    }

    #[tokio::test]
    async fn unknown_readiness_framework_is_not_found() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/readiness/frameworks/not-a-framework")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["code"], "not_found");
    }

    #[tokio::test]
    async fn authenticated_owner_can_create_and_read_a_readiness_assessment() {
        let app = app();
        let create = app
            .clone()
            .oneshot(assessment_request(
                "user-123",
                valid_assessment_payload(),
                "assessment:test-0001",
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let body: Value =
            serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let assessment_id = body["id"].as_str().unwrap();
        Uuid::parse_str(assessment_id).unwrap();
        assert_eq!(
            body["frameworkIds"],
            json!(["iso-iec-27001-2022", "soc2-tsc"])
        );
        assert_eq!(body["organization"], "Example Incorporated");
        assert_eq!(body["status"], "received");
        assert!(body["receivedAt"].as_str().is_some());
        assert!(body.get("subject").is_none());

        let read = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/readiness/assessments/{assessment_id}"))
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let read_body: Value =
            serde_json::from_slice(&read.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(read_body["id"], assessment_id);
        assert_eq!(read_body["status"], "received");
    }

    #[tokio::test]
    async fn readiness_assessments_reject_unknown_framework_ids() {
        let mut payload = valid_assessment_payload();
        payload["frameworkIds"] = json!(["iso-iec-27001-2022", "not-a-framework"]);
        let response = app()
            .oneshot(assessment_request(
                "user-123",
                payload,
                "assessment:test-0002",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["code"], "invalid_request");
    }

    #[tokio::test]
    async fn readiness_assessments_require_authentication() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/readiness/assessments")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "assessment:test-0003")
                    .body(Body::from(valid_assessment_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn readiness_assessments_are_isolated_between_subjects() {
        let app = app();
        let create = app
            .clone()
            .oneshot(assessment_request(
                "subject-a",
                valid_assessment_payload(),
                "assessment:test-0004",
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let body: Value =
            serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let assessment_id = body["id"].as_str().unwrap().to_owned();

        let own_list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/readiness/assessments")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "subject-a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(own_list.status(), StatusCode::OK);
        let own_body: Value =
            serde_json::from_slice(&own_list.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(own_body["assessments"].as_array().unwrap().len(), 1);
        assert_eq!(own_body["assessments"][0]["id"], assessment_id.as_str());

        let other_list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/readiness/assessments")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "subject-b")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other_list.status(), StatusCode::OK);
        let other_body: Value =
            serde_json::from_slice(&other_list.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(other_body["assessments"].as_array().unwrap().len(), 0);

        let other_read = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/readiness/assessments/{assessment_id}"))
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "subject-b")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other_read.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn application_markdown_is_server_owned() {
        assert!(!APPLICATION_CONTEXT_MARKDOWN.trim().is_empty());
    }

    #[test]
    fn pinned_interface_request_fixture_is_accepted() {
        let fixture = serde_json::from_str(include_str!("../fixtures/quote/v1/request.json"))
            .expect("pinned interface fixture must be valid JSON");
        parse_quote_request(fixture).expect("pinned interface fixture must satisfy quote v1");
    }

    fn valid_payload() -> Value {
        serde_json::from_str(include_str!("../fixtures/quote-v1/create-request.json")).unwrap()
    }
}
