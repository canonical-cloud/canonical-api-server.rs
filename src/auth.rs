use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::{ApiError, AppState};

type HmacSha256 = Hmac<Sha256>;
const SERVICE_TOKEN_HEADER: &str = "x-canonical-service-token";
const SERVICE_USER_ID_HEADER: &str = "x-canonical-user-id";
const SERVICE_USER_EMAIL_HEADER: &str = "x-canonical-user-email";
const COMPARISON_KEY: &[u8] = b"canonical API service-token comparison v1";

#[derive(Clone, Debug)]
pub struct Actor {
    pub user_id: Uuid,
    pub email: String,
    pub source: ActorSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorSource {
    SupabaseBearer,
    CanonicalWebService,
}

#[derive(Deserialize)]
struct SupabaseUser {
    id: Uuid,
    email: Option<String>,
}

impl FromRequestParts<AppState> for Actor {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(service_token) = parts.headers.get(SERVICE_TOKEN_HEADER) {
            let service_token = service_token.to_str().map_err(|_| ApiError::Unauthorized)?;
            if !constant_time_eq(&state.config.web_service_token, service_token) {
                return Err(ApiError::Unauthorized);
            }
            let user_id = parts
                .headers
                .get(SERVICE_USER_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<Uuid>().ok())
                .filter(|value| !value.is_nil())
                .ok_or(ApiError::Unauthorized)?;
            let email = parts
                .headers
                .get(SERVICE_USER_EMAIL_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .trim()
                .to_owned();
            if email.len() > 320 {
                return Err(ApiError::Unauthorized);
            }
            return Ok(Self {
                user_id,
                email,
                source: ActorSource::CanonicalWebService,
            });
        }

        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 8_192)
            .ok_or(ApiError::Unauthorized)?;
        let response = state
            .http
            .get(format!("{}/auth/v1/user", state.config.supabase_url))
            .header("apikey", &state.config.supabase_publishable_key)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| ApiError::Upstream)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "Supabase user lookup failed");
            return Err(ApiError::Upstream);
        }
        let user: SupabaseUser = response.json().await.map_err(|_| ApiError::Upstream)?;
        if user.id.is_nil() || user.email.as_deref().is_some_and(|email| email.len() > 320) {
            return Err(ApiError::Unauthorized);
        }
        Ok(Self {
            user_id: user.id,
            email: user.email.unwrap_or_default(),
            source: ActorSource::SupabaseBearer,
        })
    }
}

fn constant_time_eq(expected: &str, provided: &str) -> bool {
    let Ok(mut expected_mac) = <HmacSha256 as KeyInit>::new_from_slice(COMPARISON_KEY) else {
        return false;
    };
    expected_mac.update(expected.as_bytes());
    let Ok(mut provided_mac) = <HmacSha256 as KeyInit>::new_from_slice(COMPARISON_KEY) else {
        return false;
    };
    provided_mac.update(provided.as_bytes());
    let provided_tag = provided_mac.finalize().into_bytes();
    expected_mac.verify_slice(&provided_tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn service_token_comparison_is_exact() {
        let token = "0123456789abcdef0123456789abcdef";
        assert!(constant_time_eq(token, token));
        assert!(!constant_time_eq(token, "0123456789abcdef0123456789abcdee"));
        assert!(!constant_time_eq(token, "short"));
    }
}
