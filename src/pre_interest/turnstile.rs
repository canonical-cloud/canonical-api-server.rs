use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ApiError;

const VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const ACTION: &str = "canonical-pre-interest";
const USER_HOST: &str = "user.canonical.plus";
const ORG_HOST: &str = "org.canonical.plus";
const MAX_PROVIDER_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Serialize)]
struct TurnstileRequest<'a> {
    secret: &'a str,
    response: &'a str,
    idempotency_key: Uuid,
}

#[derive(Debug, Deserialize)]
struct TurnstileResponse {
    success: bool,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default, rename = "error-codes")]
    _error_codes: Vec<String>,
}

pub(super) async fn verify(
    http: &Client,
    secret: &str,
    response_token: &str,
) -> Result<String, ApiError> {
    let mut response = http
        .post(VERIFY_URL)
        .json(&TurnstileRequest {
            secret,
            response: response_token,
            idempotency_key: token_idempotency_key(response_token),
        })
        .send()
        .await
        .map_err(|_| ApiError::Unavailable)?;
    if !response.status().is_success() {
        return Err(ApiError::Unavailable);
    }
    if response.content_length().is_some_and(|length| {
        length > u64::try_from(MAX_PROVIDER_RESPONSE_BYTES).unwrap_or(u64::MAX)
    }) {
        return Err(ApiError::Unavailable);
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(ApiError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    let decision: TurnstileResponse =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::Unavailable)?;
    interpret(decision)
}

fn token_idempotency_key(token: &str) -> Uuid {
    let digest = Sha256::digest(token.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn interpret(decision: TurnstileResponse) -> Result<String, ApiError> {
    if !decision.success || decision.action.as_deref() != Some(ACTION) {
        return Err(ApiError::InvalidRequest);
    }
    let hostname = decision.hostname.ok_or(ApiError::InvalidRequest)?;
    if !matches!(hostname.as_str(), USER_HOST | ORG_HOST) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(hostname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_idempotency_is_stable_without_retaining_the_token() {
        let first = token_idempotency_key("opaque-turnstile-token");
        assert_eq!(first, token_idempotency_key("opaque-turnstile-token"));
        assert_ne!(first, token_idempotency_key("different-token"));
        assert!(!first.to_string().contains("opaque"));
    }

    #[test]
    fn decision_requires_exact_action_and_standard_source_host() {
        let accepted = interpret(TurnstileResponse {
            success: true,
            action: Some(ACTION.into()),
            hostname: Some(USER_HOST.into()),
            _error_codes: vec![],
        });
        assert_eq!(accepted.unwrap(), USER_HOST);

        for decision in [
            TurnstileResponse {
                success: false,
                action: Some(ACTION.into()),
                hostname: Some(USER_HOST.into()),
                _error_codes: vec![],
            },
            TurnstileResponse {
                success: true,
                action: Some("other".into()),
                hostname: Some(USER_HOST.into()),
                _error_codes: vec![],
            },
            TurnstileResponse {
                success: true,
                action: Some(ACTION.into()),
                hostname: Some("api.canonical.plus".into()),
                _error_codes: vec![],
            },
        ] {
            assert!(interpret(decision).is_err());
        }
    }
}
