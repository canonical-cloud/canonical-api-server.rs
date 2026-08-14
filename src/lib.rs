#![forbid(unsafe_code)]

mod contract;
mod gemini;
mod persistence;

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

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-flash";
const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";
const SUBJECT_HEADER: &str = "x-canonical-subject";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const MAX_MARKDOWN_CONTEXT_BYTES: usize = 262_144;
const MAX_CONTEXT_RECORD_BYTES: usize = 262_144;
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const QUOTE_ANALYSIS_MAX_CONCURRENCY: usize = 4;
const QUOTE_SUBMISSIONS_PER_WINDOW: usize = 5;
const QUOTE_SUBMISSIONS_GLOBAL_PER_WINDOW: usize = 500;
const QUOTE_SUBMISSION_WINDOW: Duration = Duration::from_secs(10 * 60);
const QUOTE_SUBMISSION_MAX_SUBJECTS: usize = 4_096;
const WEBSOCKET_MAX_CONNECTIONS: usize = 1_024;
const APPLICATION_CONTEXT_MARKDOWN: &str = include_str!("../context/quote-analysis.md");

#[derive(Clone)]
pub struct Config {
    pub bind_address: String,
    pub database_url: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,
    pub internal_auth_token: String,
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

        Ok(Self {
            bind_address: env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: env::var("DATABASE_URL").ok(),
            gemini_api_key,
            gemini_model,
            internal_auth_token,
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
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    InvalidGeminiModel,
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
    database: Option<DatabaseConnection>,
    events: broadcast::Sender<QuoteEventRecord>,
    gemini: Option<GeminiClient>,
    gemini_model: Arc<str>,
    idempotency: Arc<RwLock<HashMap<(String, String), MemoryOperation>>>,
    internal_auth_token: Arc<str>,
    next_event_sequence: Arc<AtomicU64>,
    quotes: Arc<RwLock<HashMap<Uuid, QuoteRecord>>>,
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
            database,
            events,
            gemini: None,
            gemini_model: gemini_model.into(),
            idempotency: Arc::new(RwLock::new(HashMap::new())),
            internal_auth_token: internal_auth_token.into(),
            next_event_sequence: Arc::new(AtomicU64::new(1)),
            quotes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_gemini(mut self, gemini: GeminiClient) -> Self {
        self.gemini = Some(gemini);
        self
    }
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
        .route("/v1/quotes", get(list_quotes).post(create_quote))
        .route("/v1/quotes/{quote_id}", get(get_quote))
        .route("/v1/quotes/{quote_id}/retry", post(retry_quote))
        .route("/v1/quotes/{quote_id}/events", get(quote_events))
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
        service: "canonical-api-server",
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn create_quote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Result<(StatusCode, Json<QuoteSubmissionResponse>), ApiError> {
    let subject = authenticate(&headers, &state.internal_auth_token)?;
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
        if let Some(operation) = state.idempotency.read().await.get(&key).cloned() {
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
    let subject = authenticate(&headers, &state.internal_auth_token)?;
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
        if let Some(operation) = state.idempotency.read().await.get(&key).cloned() {
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
    let _ = state.events.send(event);
}

async fn list_quotes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<QuoteListQuery>,
) -> Result<Json<QuoteListResponse>, ApiError> {
    let subject = authenticate(&headers, &state.internal_auth_token)?;
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
    let subject = authenticate(&headers, &state.internal_auth_token)?;
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
    let subject = authenticate(&headers, &state.internal_auth_token)?;
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

fn authenticate(headers: &HeaderMap, expected_token: &str) -> Result<String, ApiError> {
    let supplied_token = headers
        .get(INTERNAL_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let token_is_valid = supplied_token
        .map(|token| bool::from(token.as_bytes().ct_eq(expected_token.as_bytes())))
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
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 255
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
        })
        .ok_or_else(|| {
            ApiError::unauthorized("unauthorized", "authenticated subject is required")
        })?;
    Ok(subject.to_owned())
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| {
            (8..=128).contains(&value.len())
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
                })
        })
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_request",
                "Idempotency-Key must contain 8 to 128 allowed ASCII characters",
            )
        })?;
    Ok(value.to_owned())
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
            response.headers()["content-security-policy"],
            "default-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        );
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
