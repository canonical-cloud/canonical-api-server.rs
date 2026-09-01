mod model;
mod storage;
mod turnstile;

use std::time::Duration;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header::CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use reqwest::redirect::Policy;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tower_http::{
    sensitive_headers::SetSensitiveRequestHeadersLayer, timeout::TimeoutLayer,
};

use self::model::PreInterestRequest;

const PUBLIC_ROUTE: &str = "/api/v1/pre-interest";
const PUBLIC_ROUTE_ALIAS: &str = "/v1/pre-interest";
const INTERNAL_ROUTE: &str = "/internal/v1/pre-interest";
const PUBLIC_SURFACE_HEADER: &str = "x-canonical-public-surface";
const EDGE_TOKEN_HEADER: &str = "x-canonical-edge-token";
const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";
const FORWARDED_HOST_HEADER: &str = "x-forwarded-host";
const PUBLIC_SURFACE_VALUE: &str = "pre-interest";
const PUBLIC_API_HOST: &str = "api.canonical.plus";
const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone)]
struct PreInterestState {
    database: DatabaseConnection,
    edge_token: Secret,
    hmac_key: Secret,
    http: reqwest::Client,
    internal_token: Secret,
    turnstile_secret: Secret,
}

#[derive(Clone)]
pub(super) struct Secret(String);

impl Secret {
    fn parse(raw: String, name: &'static str, minimum: usize) -> Result<Self, ConfigError> {
        if raw.len() < minimum
            || raw.len() > 4096
            || raw
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(ConfigError::Invalid(name));
        }
        Ok(Self(raw))
    }

    pub(super) fn expose(&self) -> &str {
        &self.0
    }

    fn matches(&self, provided: &str) -> bool {
        bool::from(self.0.as_bytes().ct_eq(provided.as_bytes()))
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid environment variable: {0}")]
    Invalid(&'static str),
    #[error("pre-interest registration requires PostgreSQL")]
    DatabaseRequired,
    #[error("failed to build pre-interest HTTP client")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Debug, Error)]
pub(super) enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("invalid request")]
    InvalidRequest,
    #[error("dependency unavailable")]
    Unavailable,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => json_response(StatusCode::NOT_FOUND, json!({"error": "not_found"})),
            Self::Unauthorized => {
                json_response(StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}))
            }
            Self::InvalidRequest => {
                json_response(StatusCode::BAD_REQUEST, json!({"error": "invalid_request"}))
            }
            Self::Unavailable => json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "unavailable"}),
            ),
        }
    }
}

#[derive(Serialize)]
struct AcceptedResponse {
    status: &'static str,
}

pub fn router_from_env(
    database: Option<DatabaseConnection>,
) -> Result<Option<Router>, ConfigError> {
    if !parse_enabled()? {
        return Ok(None);
    }
    let database = database.ok_or(ConfigError::DatabaseRequired)?;
    let state = PreInterestState {
        database,
        edge_token: Secret::parse(
            required_env("CANONICAL_PRE_INTEREST_EDGE_TOKEN")?,
            "CANONICAL_PRE_INTEREST_EDGE_TOKEN",
            32,
        )?,
        hmac_key: Secret::parse(
            required_env("CANONICAL_PRE_INTEREST_HMAC_KEY")?,
            "CANONICAL_PRE_INTEREST_HMAC_KEY",
            32,
        )?,
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .redirect(Policy::none())
            .user_agent("canonical-api-server/pre-interest-v1")
            .build()
            .map_err(ConfigError::HttpClient)?,
        internal_token: Secret::parse(
            required_env("CANONICAL_INTERNAL_AUTH_TOKEN")?,
            "CANONICAL_INTERNAL_AUTH_TOKEN",
            32,
        )?,
        turnstile_secret: Secret::parse(
            required_env("CANONICAL_PRE_INTEREST_TURNSTILE_SECRET")?,
            "CANONICAL_PRE_INTEREST_TURNSTILE_SECRET",
            20,
        )?,
    };

    let sensitive_headers = [
        HeaderName::from_static(EDGE_TOKEN_HEADER),
        HeaderName::from_static(INTERNAL_TOKEN_HEADER),
    ];
    Ok(Some(
        Router::new()
            .route(PUBLIC_ROUTE, post(submit_public))
            .route(PUBLIC_ROUTE_ALIAS, post(submit_public))
            .route(INTERNAL_ROUTE, post(submit_internal))
            .with_state(state)
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .layer(SetSensitiveRequestHeadersLayer::new(sensitive_headers))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                Duration::from_secs(8),
            )),
    ))
}

fn parse_enabled() -> Result<bool, ConfigError> {
    match std::env::var("CANONICAL_PRE_INTEREST_ENABLED") {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(ConfigError::Invalid("CANONICAL_PRE_INTEREST_ENABLED"))
        }
        Ok(value) => match value.as_str() {
            "0" | "false" | "FALSE" => Ok(false),
            "1" | "true" | "TRUE" => Ok(true),
            _ => Err(ConfigError::Invalid("CANONICAL_PRE_INTEREST_ENABLED")),
        },
    }
}

fn required_env(name: &'static str) -> Result<String, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => Err(ConfigError::Missing(name)),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid(name)),
    }
}

async fn submit_public(
    State(state): State<PreInterestState>,
    headers: HeaderMap,
    Json(payload): Json<PreInterestRequest>,
) -> Result<Response, ApiError> {
    require_exact_header(&headers, PUBLIC_SURFACE_HEADER, PUBLIC_SURFACE_VALUE)?;
    require_secret_header(&headers, EDGE_TOKEN_HEADER, &state.edge_token)?;
    require_exact_header(&headers, FORWARDED_HOST_HEADER, PUBLIC_API_HOST)?;
    submit(state, payload).await
}

async fn submit_internal(
    State(state): State<PreInterestState>,
    headers: HeaderMap,
    Json(payload): Json<PreInterestRequest>,
) -> Result<Response, ApiError> {
    require_secret_header(&headers, INTERNAL_TOKEN_HEADER, &state.internal_token)?;
    submit(state, payload).await
}

async fn submit(
    state: PreInterestState,
    payload: PreInterestRequest,
) -> Result<Response, ApiError> {
    let normalized = model::normalize_request(payload)?;
    let verified_host = turnstile::verify(
        &state.http,
        state.turnstile_secret.expose(),
        &normalized.turnstile_token,
    )
    .await?;
    let accepted = model::accept_request(normalized, verified_host, &state.hmac_key)?;
    storage::persist(&state.database, &accepted).await?;
    Ok(json_response(
        StatusCode::ACCEPTED,
        serde_json::to_value(AcceptedResponse { status: "accepted" })
            .map_err(|_| ApiError::Unavailable)?,
    ))
}

fn require_exact_header(
    headers: &HeaderMap,
    name: &'static str,
    expected: &'static str,
) -> Result<(), ApiError> {
    let value = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::NotFound)?;
    if value == expected {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn require_secret_header(
    headers: &HeaderMap,
    name: &'static str,
    expected: &Secret,
) -> Result<(), ApiError> {
    let provided = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    if expected.matches(provided) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

fn json_response(status: StatusCode, value: JsonValue) -> Response {
    let body = serde_json::to_vec(&value)
        .unwrap_or_else(|_| b"{\"error\":\"unavailable\"}".to_vec());
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_reject_normalization_and_compare_without_string_logging() {
        assert!(Secret::parse("x".repeat(32), "TEST", 32).is_ok());
        assert!(Secret::parse(format!("{}\n", "x".repeat(32)), "TEST", 32).is_err());
        let secret = Secret::parse("x".repeat(32), "TEST", 32).unwrap();
        assert!(secret.matches(&"x".repeat(32)));
        assert!(!secret.matches(&"y".repeat(32)));
    }

    #[test]
    fn accepted_response_is_uniform_and_cache_prohibited() {
        let response = json_response(
            StatusCode::ACCEPTED,
            serde_json::to_value(AcceptedResponse { status: "accepted" }).unwrap(),
        );
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
}
