#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, RwLock};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-pro";
const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";
const SUBJECT_HEADER: &str = "x-canonical-subject";

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_address: String,
    pub database_url: Option<String>,
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

        Ok(Self {
            bind_address: env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: env::var("DATABASE_URL").ok(),
            gemini_model: env::var("GEMINI_MODEL")
                .unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.into()),
            internal_auth_token,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    MissingInternalAuthToken,
    WeakInternalAuthToken,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingInternalAuthToken => {
                formatter.write_str("CANONICAL_INTERNAL_AUTH_TOKEN is required")
            }
            Self::WeakInternalAuthToken => formatter.write_str(
                "CANONICAL_INTERNAL_AUTH_TOKEN must contain at least 32 non-whitespace bytes",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone)]
pub struct AppState {
    database: Option<DatabaseConnection>,
    events: broadcast::Sender<QuoteEvent>,
    gemini_model: Arc<str>,
    internal_auth_token: Arc<str>,
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
            database,
            events,
            gemini_model: gemini_model.into(),
            internal_auth_token: internal_auth_token.into(),
            quotes: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[must_use]
pub fn build_router(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/quotes", post(create_quote))
        .route("/v1/quotes/{quote_id}", get(get_quote))
        .route("/v1/quotes/{quote_id}/events", get(quote_events))
        .with_state(state)
        .layer(SetSensitiveRequestHeadersLayer::new([
            axum::http::header::AUTHORIZATION,
            HeaderName::from_static(INTERNAL_TOKEN_HEADER),
        ]))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(TraceLayer::new_for_http())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        database_configured: state.database.is_some(),
        gemini_model: state.gemini_model.to_string(),
        service: "canonical-api-server",
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn create_quote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateQuoteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let subject = authenticate(&headers, &state.internal_auth_token)?;
    request.validate()?;

    let quote_id = Uuid::new_v4();
    let record = QuoteRecord {
        context_record_id: request.context_record_id,
        frameworks: request.frameworks,
        gemini_model: state.gemini_model.to_string(),
        organization_name: request.organization.legal_name,
        owner_subject: subject.clone(),
        persistence: if state.database.is_some() {
            "database-pending"
        } else {
            "memory-only"
        },
        quote_id,
        status: "queued",
    };

    state.quotes.write().await.insert(quote_id, record.clone());
    let _ = state.events.send(QuoteEvent {
        owner_subject: subject,
        quote_id,
        status: "queued".into(),
    });

    Ok((StatusCode::ACCEPTED, Json(record)))
}

async fn get_quote(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<QuoteRecord>, ApiError> {
    let subject = authenticate(&headers, &state.internal_auth_token)?;
    let record = state
        .quotes
        .read()
        .await
        .get(&quote_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("quote_not_found", "quote was not found"))?;
    if record.owner_subject != subject {
        return Err(ApiError::not_found("quote_not_found", "quote was not found"));
    }
    Ok(Json(record))
}

async fn quote_events(
    State(state): State<AppState>,
    Path(quote_id): Path<Uuid>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let subject = authenticate(&headers, &state.internal_auth_token)?;
    let record = state
        .quotes
        .read()
        .await
        .get(&quote_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("quote_not_found", "quote was not found"))?;
    if record.owner_subject != subject {
        return Err(ApiError::not_found("quote_not_found", "quote was not found"));
    }

    Ok(upgrade.on_upgrade(move |socket| stream_quote_events(socket, state, quote_id, subject)))
}

async fn stream_quote_events(
    mut socket: WebSocket,
    state: AppState,
    quote_id: Uuid,
    owner_subject: String,
) {
    let mut events = state.events.subscribe();
    if let Some(record) = state.quotes.read().await.get(&quote_id).cloned() {
        if send_json(&mut socket, &record).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) if event.quote_id == quote_id && event.owner_subject == owner_subject => {
                        if send_json(&mut socket, &event).await.is_err() {
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
    socket.send(Message::Text(payload.into())).await.map_err(|_| ())
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
            "invalid_internal_auth",
            "trusted proxy authentication failed",
        ));
    }

    let subject = headers
        .get(SUBJECT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .ok_or_else(|| {
            ApiError::unauthorized("missing_subject", "authenticated subject is required")
        })?;
    Ok(subject.to_owned())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateQuoteRequest {
    pub context_record_id: Uuid,
    pub frameworks: Vec<String>,
    pub markdown_context: String,
    pub notes: Option<String>,
    pub organization: OrganizationInput,
    pub target_date: Option<String>,
}

impl CreateQuoteRequest {
    fn validate(&self) -> Result<(), ApiError> {
        if self.organization.legal_name.trim().is_empty() {
            return Err(ApiError::bad_request(
                "invalid_organization",
                "organization.legal_name is required",
            ));
        }
        if self.organization.employee_count == 0 {
            return Err(ApiError::bad_request(
                "invalid_employee_count",
                "organization.employee_count must be greater than zero",
            ));
        }
        if self.frameworks.is_empty() || self.frameworks.len() > 16 {
            return Err(ApiError::bad_request(
                "invalid_frameworks",
                "between 1 and 16 frameworks are required",
            ));
        }
        if self.markdown_context.trim().is_empty() || self.markdown_context.len() > 262_144 {
            return Err(ApiError::bad_request(
                "invalid_markdown_context",
                "markdown_context must contain 1 to 262144 bytes",
            ));
        }
        if self.notes.as_deref().is_some_and(|notes| notes.len() > 32_768) {
            return Err(ApiError::bad_request(
                "notes_too_large",
                "notes must not exceed 32768 bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationInput {
    pub employee_count: u32,
    pub industry: String,
    pub legal_name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct QuoteRecord {
    pub context_record_id: Uuid,
    pub frameworks: Vec<String>,
    pub gemini_model: String,
    pub organization_name: String,
    #[serde(skip_serializing)]
    pub owner_subject: String,
    pub persistence: &'static str,
    pub quote_id: Uuid,
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct QuoteEvent {
    #[serde(skip_serializing)]
    owner_subject: String,
    quote_id: Uuid,
    status: String,
}

#[derive(Serialize)]
struct HealthResponse {
    database_configured: bool,
    gemini_model: String,
    service: &'static str,
    status: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

#[derive(Debug)]
struct ApiError {
    body: ErrorBody,
    status: StatusCode,
}

impl ApiError {
    const fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            body: ErrorBody { code, message },
            status: StatusCode::BAD_REQUEST,
        }
    }

    const fn not_found(code: &'static str, message: &'static str) -> Self {
        Self {
            body: ErrorBody { code, message },
            status: StatusCode::NOT_FOUND,
        }
    }

    const fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self {
            body: ErrorBody { code, message },
            status: StatusCode::UNAUTHORIZED,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
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

    #[tokio::test]
    async fn health_is_public_and_reports_database_state() {
        let response = app()
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &response.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap();
        assert_eq!(body["database_configured"], false);
        assert_eq!(body["gemini_model"], DEFAULT_GEMINI_MODEL);
    }

    #[tokio::test]
    async fn quote_creation_requires_trusted_proxy_headers() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/quotes")
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
                    .uri("/v1/quotes")
                    .header("content-type", "application/json")
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::from(valid_payload().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::ACCEPTED);
        let body: Value = serde_json::from_slice(
            &create.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap();
        let quote_id = body["quote_id"].as_str().unwrap();
        Uuid::parse_str(quote_id).unwrap();

        let read = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/quotes/{quote_id}"))
                    .header("x-canonical-internal-token", TOKEN)
                    .header("x-canonical-subject", "user-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
    }

    fn valid_payload() -> Value {
        json!({
            "context_record_id": Uuid::new_v4(),
            "frameworks": ["soc2", "hipaa"],
            "markdown_context": "# Product\nA hosted control plane.",
            "notes": "Initial estimate",
            "organization": {
                "employee_count": 42,
                "industry": "Software",
                "legal_name": "Example Incorporated"
            },
            "target_date": "2027-01-15"
        })
    }
}
