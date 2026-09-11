use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::redirect::Policy;
use serde::Serialize;
use sha2::Sha256;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::time::sleep;

use crate::{AssessmentEventRecord, QuoteEventRecord};

const DELIVERY_ATTEMPTS: usize = 3;
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct WebhookDispatcher {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    secret: Arc<[u8]>,
}

impl fmt::Debug for WebhookDispatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookDispatcher")
            .field(
                "endpoint_origin",
                &self.endpoint.origin().ascii_serialization(),
            )
            .field("secret", &"[redacted]")
            .finish()
    }
}

impl WebhookDispatcher {
    pub fn new(endpoint: &str, secret: impl AsRef<[u8]>) -> Result<Self, WebhookBuildError> {
        let endpoint = reqwest::Url::parse(endpoint).map_err(|_| WebhookBuildError::InvalidUrl)?;
        validate_endpoint(&endpoint)?;
        let secret = secret.as_ref();
        if secret
            .iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .count()
            < 32
        {
            return Err(WebhookBuildError::WeakSecret);
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(DELIVERY_TIMEOUT)
            .user_agent(concat!("canonical-api-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| WebhookBuildError::Client)?;
        Ok(Self {
            client,
            endpoint,
            secret: Arc::from(secret),
        })
    }

    pub(crate) async fn deliver(&self, event: QuoteEventRecord) -> Result<(), DeliveryError> {
        self.dispatch(WebhookEvent::from(event)).await
    }

    pub(crate) async fn deliver_assessment(
        &self,
        event: AssessmentEventRecord,
    ) -> Result<(), DeliveryError> {
        self.dispatch(WebhookEvent::from(event)).await
    }

    async fn dispatch(&self, event: WebhookEvent) -> Result<(), DeliveryError> {
        let body = serde_json::to_vec(&event).map_err(|_| DeliveryError::Serialize)?;
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature = signature(&self.secret, &timestamp, &body);

        for attempt in 0..DELIVERY_ATTEMPTS {
            let response = self
                .client
                .post(self.endpoint.clone())
                .header("content-type", "application/json")
                .header("x-canonical-webhook-id", &event.id)
                .header("x-canonical-webhook-timestamp", &timestamp)
                .header("x-canonical-webhook-signature", &signature)
                .body(body.clone())
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response)
                    if response.status().is_client_error()
                        && !matches!(response.status().as_u16(), 408 | 429) =>
                {
                    return Err(DeliveryError::Rejected(response.status().as_u16()));
                }
                Ok(response) if attempt + 1 == DELIVERY_ATTEMPTS => {
                    return Err(DeliveryError::Unavailable(Some(response.status().as_u16())));
                }
                Err(_) if attempt + 1 == DELIVERY_ATTEMPTS => {
                    return Err(DeliveryError::Unavailable(None));
                }
                Ok(_) | Err(_) => {
                    sleep(match attempt {
                        0 => Duration::from_millis(250),
                        _ => Duration::from_secs(1),
                    })
                    .await;
                }
            }
        }
        Err(DeliveryError::Unavailable(None))
    }
}

fn validate_endpoint(endpoint: &reqwest::Url) -> Result<(), WebhookBuildError> {
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
        || endpoint.query().is_some()
    {
        return Err(WebhookBuildError::UnsafeUrl);
    }
    let host = endpoint.host_str().ok_or(WebhookBuildError::InvalidUrl)?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && loopback) {
        return Err(WebhookBuildError::InsecureUrl);
    }
    Ok(())
}

fn signature(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut encoded = String::with_capacity(3 + bytes.len() * 2);
    encoded.push_str("v1=");
    for byte in bytes {
        use fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookEvent {
    data: WebhookEventData,
    id: String,
    occurred_at: String,
    source: &'static str,
    spec_version: &'static str,
    #[serde(rename = "type")]
    event_type: &'static str,
}

impl From<QuoteEventRecord> for WebhookEvent {
    fn from(event: QuoteEventRecord) -> Self {
        let status = match event.status.as_str() {
            "completed" => "ready".to_string(),
            _ => event.status,
        };
        Self {
            id: format!("quote:{}:{}", event.quote_id, event.sequence),
            occurred_at: event.occurred_at,
            source: "api.canonical.plus",
            spec_version: "1.0",
            event_type: "canonical.quote.status.changed",
            data: WebhookEventData::Quote(QuoteWebhookData {
                quote_id: event.quote_id.to_string(),
                sequence: event.sequence,
                status,
            }),
        }
    }
}

impl From<AssessmentEventRecord> for WebhookEvent {
    fn from(event: AssessmentEventRecord) -> Self {
        Self {
            id: format!("readiness-assessment:{}", event.assessment_id),
            occurred_at: event.occurred_at,
            source: "api.canonical.plus",
            spec_version: "1.0",
            event_type: "readiness.assessment.received",
            data: WebhookEventData::Assessment(AssessmentWebhookData {
                assessment_id: event.assessment_id.to_string(),
                framework_ids: event.framework_ids,
                status: event.status,
            }),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum WebhookEventData {
    Assessment(AssessmentWebhookData),
    Quote(QuoteWebhookData),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssessmentWebhookData {
    assessment_id: String,
    framework_ids: Vec<String>,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuoteWebhookData {
    quote_id: String,
    sequence: u64,
    status: String,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum WebhookBuildError {
    #[error("webhook HTTP client could not be initialized")]
    Client,
    #[error("webhook URL must use HTTPS, except for an HTTP loopback target")]
    InsecureUrl,
    #[error("webhook URL is invalid")]
    InvalidUrl,
    #[error("webhook URL must not contain credentials, query parameters, or a fragment")]
    UnsafeUrl,
    #[error("webhook secret must contain at least 32 non-whitespace bytes")]
    WeakSecret,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum DeliveryError {
    #[error("webhook endpoint rejected the event with HTTP {0}")]
    Rejected(u16),
    #[error("webhook event could not be serialized")]
    Serialize,
    #[error("webhook endpoint remained unavailable after bounded retries (last status: {0:?})")]
    Unavailable(Option<u16>),
}

#[cfg(test)]
mod tests {
    use super::{signature, WebhookBuildError, WebhookDispatcher, WebhookEvent};
    use crate::{AssessmentEventRecord, QuoteEventRecord};
    use uuid::Uuid;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn signatures_are_versioned_and_bind_timestamp_and_exact_body() {
        let first = signature(SECRET.as_bytes(), "1700000000", br#"{"status":"ready"}"#);
        let replayed_at_another_time =
            signature(SECRET.as_bytes(), "1700000001", br#"{"status":"ready"}"#);
        let altered = signature(SECRET.as_bytes(), "1700000000", br#"{"status":"failed"}"#);
        assert!(first.starts_with("v1="));
        assert_eq!(first.len(), 67);
        assert_ne!(first, replayed_at_another_time);
        assert_ne!(first, altered);
    }

    #[test]
    fn endpoint_policy_rejects_credential_leaks_and_non_tls_network_targets() {
        assert!(matches!(
            WebhookDispatcher::new("http://example.com/events", SECRET),
            Err(WebhookBuildError::InsecureUrl)
        ));
        assert!(matches!(
            WebhookDispatcher::new("https://user:password@example.com/events", SECRET),
            Err(WebhookBuildError::UnsafeUrl)
        ));
        assert!(matches!(
            WebhookDispatcher::new("https://example.com/events?token=secret", SECRET),
            Err(WebhookBuildError::UnsafeUrl)
        ));
        assert!(matches!(
            WebhookDispatcher::new(
                "https://example.com/events",
                "x                               "
            ),
            Err(WebhookBuildError::WeakSecret)
        ));
        WebhookDispatcher::new("https://hooks.example.com/canonical", SECRET).unwrap();
        WebhookDispatcher::new("http://127.0.0.1:9090/canonical", SECRET).unwrap();
    }

    #[test]
    fn webhook_debug_output_redacts_secret_and_path() {
        let dispatcher =
            WebhookDispatcher::new("https://hooks.example.com/customer/private/path", SECRET)
                .unwrap();
        let rendered = format!("{dispatcher:?}");
        assert!(rendered.contains("https://hooks.example.com"));
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains("customer/private/path"));
    }

    #[test]
    fn webhook_uses_stable_ids_and_the_public_ready_status() {
        let quote_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let event = WebhookEvent::from(QuoteEventRecord {
            owner_subject: "must-not-be-serialized".into(),
            quote_id,
            sequence: 42,
            status: "completed".into(),
            occurred_at: "2026-08-22T20:00:00Z".into(),
        });
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["id"], format!("quote:{quote_id}:42"));
        assert_eq!(json["data"]["status"], "ready");
        assert!(json.to_string().find("must-not-be-serialized").is_none());
    }

    #[test]
    fn assessment_webhooks_use_stable_ids_and_the_received_status() {
        let assessment_id = Uuid::parse_str("22222222-3333-4444-8555-666666666666").unwrap();
        let event = WebhookEvent::from(AssessmentEventRecord {
            assessment_id,
            framework_ids: vec!["soc2-tsc".into()],
            occurred_at: "2026-08-22T20:00:00Z".into(),
            status: "received".into(),
        });
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["id"], format!("readiness-assessment:{assessment_id}"));
        assert_eq!(json["type"], "readiness.assessment.received");
        assert_eq!(json["data"]["frameworkIds"][0], "soc2-tsc");
        assert_eq!(json["data"]["status"], "received");
    }
}
