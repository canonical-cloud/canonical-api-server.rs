use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderMap, StatusCode};
use reqwest::Url;
use subtle::ConstantTimeEq;

use crate::ApiError;

const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";
const SUBJECT_HEADER: &str = "x-canonical-subject";
const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct Authenticator {
    internal_token: Arc<str>,
    shared_auth: Option<SharedAuthVerifier>,
}

impl Authenticator {
    pub(crate) fn new(
        internal_token: impl Into<Arc<str>>,
        shared_auth_verify_url: Option<String>,
    ) -> Result<Self, crate::ConfigError> {
        let shared_auth = shared_auth_verify_url
            .map(SharedAuthVerifier::new)
            .transpose()?;
        Ok(Self {
            internal_token: internal_token.into(),
            shared_auth,
        })
    }

    pub(crate) async fn authenticate(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        if let Some(token) = headers
            .get(INTERNAL_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            let valid = bool::from(token.as_bytes().ct_eq(self.internal_token.as_bytes()));
            if !valid {
                return Err(ApiError::unauthorized(
                    "invalid_internal_auth",
                    "trusted proxy authentication failed",
                ));
            }
            return subject_header(headers);
        }

        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= MAX_TOKEN_BYTES)
            .ok_or_else(|| {
                ApiError::unauthorized("authentication_required", "authentication is required")
            })?;

        let verifier = self.shared_auth.as_ref().ok_or_else(|| {
            ApiError::service_unavailable(
                "shared_auth_unavailable",
                "direct API authentication is not configured",
            )
        })?;
        verifier.verify(bearer).await
    }
}

fn subject_header(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
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
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::unauthorized("missing_subject", "authenticated subject is required")
        })
}

#[derive(Clone)]
struct SharedAuthVerifier {
    client: reqwest::Client,
    verify_url: Url,
}

impl SharedAuthVerifier {
    fn new(raw_url: String) -> Result<Self, crate::ConfigError> {
        let verify_url = raw_url
            .parse::<Url>()
            .map_err(|_| crate::ConfigError::InvalidSharedAuthVerifyUrl)?;
        validate_verify_url(&verify_url)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| crate::ConfigError::InvalidSharedAuthVerifyUrl)?;
        Ok(Self { client, verify_url })
    }

    async fn verify(&self, token: &str) -> Result<String, ApiError> {
        let response = self
            .client
            .get(self.verify_url.clone())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| {
                ApiError::service_unavailable(
                    "shared_auth_unavailable",
                    "authentication service is temporarily unavailable",
                )
            })?;

        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(ApiError::unauthorized(
                    "invalid_bearer",
                    "bearer authentication failed",
                ));
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(ApiError::service_unavailable(
                    "shared_auth_busy",
                    "authentication service is temporarily busy",
                ));
            }
            _ => {
                return Err(ApiError::service_unavailable(
                    "shared_auth_unavailable",
                    "authentication service is temporarily unavailable",
                ));
            }
        }

        let headers = response.headers();
        let subject = required_header(headers, "x-auth-user-id")?;
        let _provider = required_header(headers, "x-auth-provider")?;
        let _provider_tenant = required_header(headers, "x-auth-provider-tenant")?;
        if subject.len() > 255
            || subject
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')))
        {
            return Err(ApiError::unauthorized(
                "invalid_subject",
                "verified subject is invalid",
            ));
        }
        Ok(subject)
    }
}

fn required_header(
    headers: &reqwest::header::HeaderMap,
    name: &'static str,
) -> Result<String, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::unauthorized(
                "invalid_shared_auth_response",
                "authentication response is invalid",
            )
        })
}

fn validate_verify_url(url: &Url) -> Result<(), crate::ConfigError> {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || url.path() != "/shared-auth/auth/verify"
    {
        return Err(crate::ConfigError::InvalidSharedAuthVerifyUrl);
    }
    let host = url.host_str().unwrap_or_default();
    let local = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".svc")
        || host.contains(".svc.");
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(crate::ConfigError::InvalidSharedAuthVerifyUrl);
    }
    Ok(())
}
