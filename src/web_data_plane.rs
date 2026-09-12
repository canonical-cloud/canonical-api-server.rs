//! Fail-closed contracts for the four supported web-to-API data paths.
//!
//! Service credentials are transport configuration and never fields in the
//! product request envelope. The authenticated end-user subject remains
//! distinct across every mode.

use std::collections::BTreeMap;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const WEB_API_CONTRACT: &str = "canonical-cloud/web-api/v1";
pub const API_AUDIENCE: &str = "canonical-plus-api";
pub const DIRECT_DATABASE_ROLE: &str = "canonical_cloud__quote__web_ro";
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_DEDUPE_KEY_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebApiMode {
    DirectReadOnlyDatabase,
    StatelessHttp,
    StatefulMtlsTcp,
    JetStreamAsync,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataOperation {
    Read,
    Write,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebApiRequest {
    pub contract: String,
    pub request_id: String,
    pub tenant_id: String,
    pub subject: String,
    pub audience: String,
    pub operation: DataOperation,
    pub resource: String,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebApiResponse {
    pub contract: String,
    pub request_id: String,
    pub status: u16,
    pub payload: Value,
}

impl WebApiRequest {
    pub fn validate_for(&self, mode: WebApiMode) -> Result<(), DataPlaneError> {
        if self.contract != WEB_API_CONTRACT {
            return Err(DataPlaneError::Contract);
        }
        validate_identifier("request_id", &self.request_id, 128)?;
        validate_identifier("tenant_id", &self.tenant_id, 128)?;
        validate_identifier("subject", &self.subject, 255)?;
        if self.audience != API_AUDIENCE {
            return Err(DataPlaneError::Audience);
        }
        validate_resource(&self.resource)?;
        if mode == WebApiMode::DirectReadOnlyDatabase && self.operation != DataOperation::Read {
            return Err(DataPlaneError::DirectDatabaseWrite);
        }
        if mode == WebApiMode::JetStreamAsync {
            validate_dedupe_key(self.dedupe_key.as_deref())?;
        }
        let encoded = serde_json::to_vec(self).map_err(|_| DataPlaneError::Serialization)?;
        if encoded.len() > MAX_REQUEST_BYTES {
            return Err(DataPlaneError::RequestTooLarge {
                actual: encoded.len(),
                maximum: MAX_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

impl WebApiResponse {
    pub fn encode_bounded(&self) -> Result<Vec<u8>, DataPlaneError> {
        if self.contract != WEB_API_CONTRACT {
            return Err(DataPlaneError::Contract);
        }
        validate_identifier("request_id", &self.request_id, 128)?;
        let encoded = serde_json::to_vec(self).map_err(|_| DataPlaneError::Serialization)?;
        if encoded.len() > MAX_RESPONSE_BYTES {
            return Err(DataPlaneError::ResponseTooLarge {
                actual: encoded.len(),
                maximum: MAX_RESPONSE_BYTES,
            });
        }
        Ok(encoded)
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectDatabasePolicy {
    pub role_name: String,
    pub transaction_read_only: bool,
    pub forced_row_level_security: bool,
    pub statement_timeout_ms: u64,
    pub lock_timeout_ms: u64,
    pub row_limit: u32,
}

impl DirectDatabasePolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        if self.role_name != DIRECT_DATABASE_ROLE
            || !self.transaction_read_only
            || !self.forced_row_level_security
            || !(1..=5_000).contains(&self.statement_timeout_ms)
            || !(1..=1_000).contains(&self.lock_timeout_ms)
            || !(1..=1_000).contains(&self.row_limit)
        {
            return Err(DataPlaneError::UnsafeDirectDatabasePolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatelessHttpPolicy {
    pub base_url: String,
    pub service_credential_ref: String,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub redirects_enabled: bool,
}

impl StatelessHttpPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        if !safe_service_url(&self.base_url)
            || !safe_secret_reference(&self.service_credential_ref)
            || self.redirects_enabled
            || !(1..=2_000).contains(&self.connect_timeout_ms)
            || !(1..=10_000).contains(&self.request_timeout_ms)
            || !(1..=MAX_REQUEST_BYTES).contains(&self.max_request_bytes)
            || !(1..=MAX_RESPONSE_BYTES).contains(&self.max_response_bytes)
        {
            return Err(DataPlaneError::UnsafeHttpPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatefulMtlsTcpPolicy {
    pub address: String,
    pub server_name: String,
    pub ca_bundle_ref: String,
    pub client_certificate_ref: String,
    pub client_private_key_ref: String,
    pub tls_minimum_version: String,
    pub connect_timeout_ms: u64,
    pub io_timeout_ms: u64,
    pub max_frame_bytes: usize,
}

impl StatefulMtlsTcpPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        let references = [
            self.ca_bundle_ref.as_str(),
            self.client_certificate_ref.as_str(),
            self.client_private_key_ref.as_str(),
        ];
        if !valid_socket_address(&self.address)
            || !valid_dns_name(&self.server_name)
            || references.iter().any(|value| !safe_secret_reference(value))
            || self.tls_minimum_version != "1.3"
            || !(1..=2_000).contains(&self.connect_timeout_ms)
            || !(1..=10_000).contains(&self.io_timeout_ms)
            || !(1..=MAX_FRAME_BYTES).contains(&self.max_frame_bytes)
        {
            return Err(DataPlaneError::UnsafeMtlsPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JetStreamPolicy {
    pub stream: String,
    pub request_subject: String,
    pub status_subject: String,
    pub durable_consumer: String,
    pub outbox_table: String,
    pub inbox_table: String,
    pub status_table: String,
    pub ack_timeout_ms: u64,
    pub dedupe_window_seconds: u64,
    pub max_message_bytes: usize,
    pub max_deliveries: u16,
}

impl JetStreamPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        let names = [
            self.stream.as_str(),
            self.request_subject.as_str(),
            self.status_subject.as_str(),
            self.durable_consumer.as_str(),
            self.outbox_table.as_str(),
            self.inbox_table.as_str(),
            self.status_table.as_str(),
        ];
        if names.iter().any(|value| !valid_route_name(value))
            || self.request_subject == self.status_subject
            || !(1_000..=120_000).contains(&self.ack_timeout_ms)
            || !(60..=604_800).contains(&self.dedupe_window_seconds)
            || !(1..=MAX_REQUEST_BYTES).contains(&self.max_message_bytes)
            || !(1..=25).contains(&self.max_deliveries)
        {
            return Err(DataPlaneError::UnsafeJetStreamPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncStatus {
    Pending,
    Published,
    Processing,
    Succeeded,
    Failed,
    DeadLetter,
}

impl AsyncStatus {
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Published)
                | (Self::Published, Self::Processing)
                | (Self::Processing, Self::Succeeded | Self::Failed)
                | (Self::Failed, Self::Published | Self::DeadLetter)
        )
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsyncReceipt {
    pub operation_id: String,
    pub dedupe_key: String,
    pub status: AsyncStatus,
    pub attempt: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl AsyncReceipt {
    /// Returns the next immutable receipt only when the durable async state
    /// machine permits the transition. Persistence and message publication are
    /// deliberately outside this pure transformation.
    pub fn transition(self, next: AsyncStatus) -> Result<Self, DataPlaneError> {
        if !self.status.can_transition_to(next) {
            return Err(DataPlaneError::InvalidStatusTransition {
                from: self.status,
                to: next,
            });
        }
        Ok(Self {
            status: next,
            attempt: self
                .attempt
                .saturating_add(u16::from(next == AsyncStatus::Processing)),
            ..self
        })
    }
}

pub fn encode_frame(payload: &[u8], maximum: usize) -> Result<Vec<u8>, DataPlaneError> {
    let maximum = maximum.min(MAX_FRAME_BYTES);
    if payload.is_empty() || payload.len() > maximum || payload.len() > u32::MAX as usize {
        return Err(DataPlaneError::FrameSize);
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8], maximum: usize) -> Result<&[u8], DataPlaneError> {
    let length = frame
        .get(..4)
        .and_then(|prefix| <[u8; 4]>::try_from(prefix).ok())
        .map(u32::from_be_bytes)
        .ok_or(DataPlaneError::FrameSize)? as usize;
    let maximum = maximum.min(MAX_FRAME_BYTES);
    if length == 0 || length > maximum || frame.len() != 4 + length {
        return Err(DataPlaneError::FrameSize);
    }
    Ok(&frame[4..])
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), DataPlaneError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DataPlaneError::InvalidIdentifier { field });
    }
    Ok(())
}

fn validate_resource(value: &str) -> Result<(), DataPlaneError> {
    if value.is_empty()
        || value.len() > 256
        || !value.starts_with('/')
        || value.contains("..")
        || value.contains(['?', '#'])
        || value.chars().any(char::is_control)
    {
        return Err(DataPlaneError::InvalidResource);
    }
    Ok(())
}

fn validate_dedupe_key(value: Option<&str>) -> Result<(), DataPlaneError> {
    let Some(value) = value else {
        return Err(DataPlaneError::MissingDedupeKey);
    };
    validate_identifier("dedupe_key", value, MAX_DEDUPE_KEY_BYTES)
}

fn safe_service_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "https"
        || (url.scheme() == "http"
            && (matches!(host, "localhost" | "127.0.0.1" | "::1")
                || host.ends_with(".svc.cluster.local")))
}

fn safe_secret_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
}

fn valid_socket_address(value: &str) -> bool {
    value.len() <= 255
        && value
            .rsplit_once(':')
            .is_some_and(|(host, port)| valid_dns_name(host) && port.parse::<u16>().is_ok())
}

fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_route_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Debug, Error, PartialEq)]
pub enum DataPlaneError {
    #[error("unsupported web/API contract")]
    Contract,
    #[error("request audience does not match this API")]
    Audience,
    #[error("invalid {field}")]
    InvalidIdentifier { field: &'static str },
    #[error("invalid resource")]
    InvalidResource,
    #[error("direct database mode cannot perform writes")]
    DirectDatabaseWrite,
    #[error("JetStream mode requires a dedupe key")]
    MissingDedupeKey,
    #[error("request is {actual} bytes; maximum is {maximum}")]
    RequestTooLarge { actual: usize, maximum: usize },
    #[error("response is {actual} bytes; maximum is {maximum}")]
    ResponseTooLarge { actual: usize, maximum: usize },
    #[error("serialization failed")]
    Serialization,
    #[error("unsafe direct database policy")]
    UnsafeDirectDatabasePolicy,
    #[error("unsafe stateless HTTP policy")]
    UnsafeHttpPolicy,
    #[error("unsafe stateful mTLS policy")]
    UnsafeMtlsPolicy,
    #[error("unsafe JetStream policy")]
    UnsafeJetStreamPolicy,
    #[error("invalid frame size")]
    FrameSize,
    #[error("invalid async status transition from {from:?} to {to:?}")]
    InvalidStatusTransition { from: AsyncStatus, to: AsyncStatus },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: DataOperation) -> WebApiRequest {
        WebApiRequest {
            contract: WEB_API_CONTRACT.to_string(),
            request_id: "req-01".to_string(),
            tenant_id: "tenant-01".to_string(),
            subject: "shared-auth:user-01".to_string(),
            audience: API_AUDIENCE.to_string(),
            operation,
            resource: "/quotes".to_string(),
            payload: serde_json::json!({"limit": 25}),
            dedupe_key: Some("tenant-01:req-01".to_string()),
        }
    }

    #[test]
    fn direct_database_is_read_only_with_the_exact_role() {
        request(DataOperation::Read)
            .validate_for(WebApiMode::DirectReadOnlyDatabase)
            .unwrap();
        assert_eq!(
            request(DataOperation::Write).validate_for(WebApiMode::DirectReadOnlyDatabase),
            Err(DataPlaneError::DirectDatabaseWrite)
        );
        DirectDatabasePolicy {
            role_name: DIRECT_DATABASE_ROLE.to_string(),
            transaction_read_only: true,
            forced_row_level_security: true,
            statement_timeout_ms: 2_000,
            lock_timeout_ms: 500,
            row_limit: 100,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn stateless_http_is_bounded_and_never_redirects() {
        StatelessHttpPolicy {
            base_url: "https://canonical-api.internal".to_string(),
            service_credential_ref: "secret/canonical-api-service-auth".to_string(),
            connect_timeout_ms: 500,
            request_timeout_ms: 3_000,
            max_request_bytes: MAX_REQUEST_BYTES,
            max_response_bytes: MAX_RESPONSE_BYTES,
            redirects_enabled: false,
        }
        .validate()
        .unwrap();
        assert_eq!(
            StatelessHttpPolicy {
                base_url: "https://user@canonical-api.internal".to_string(),
                service_credential_ref: "secret/canonical-api-service-auth".to_string(),
                connect_timeout_ms: 500,
                request_timeout_ms: 3_000,
                max_request_bytes: MAX_REQUEST_BYTES,
                max_response_bytes: MAX_RESPONSE_BYTES,
                redirects_enabled: false,
            }
            .validate(),
            Err(DataPlaneError::UnsafeHttpPolicy)
        );
    }

    #[test]
    fn stateful_tcp_is_mutually_authenticated_and_framed() {
        StatefulMtlsTcpPolicy {
            address: "canonical-api.internal:7443".to_string(),
            server_name: "canonical-api.internal".to_string(),
            ca_bundle_ref: "secret/api-ca".to_string(),
            client_certificate_ref: "secret/web-client-cert".to_string(),
            client_private_key_ref: "secret/web-client-key".to_string(),
            tls_minimum_version: "1.3".to_string(),
            connect_timeout_ms: 500,
            io_timeout_ms: 3_000,
            max_frame_bytes: MAX_FRAME_BYTES,
        }
        .validate()
        .unwrap();

        let frame = encode_frame(b"typed request", 64).unwrap();
        assert_eq!(decode_frame(&frame, 64).unwrap(), b"typed request");
        let mut trailing = frame.clone();
        trailing.push(0);
        assert_eq!(decode_frame(&trailing, 64), Err(DataPlaneError::FrameSize));
    }

    #[test]
    fn jetstream_requires_durable_dedupe_and_monotonic_status() {
        JetStreamPolicy {
            stream: "CANONICAL_WEB_API".to_string(),
            request_subject: "canonical.web_api.request".to_string(),
            status_subject: "canonical.web_api.status".to_string(),
            durable_consumer: "canonical_api_workers".to_string(),
            outbox_table: "web_api_outbox".to_string(),
            inbox_table: "web_api_inbox".to_string(),
            status_table: "web_api_status".to_string(),
            ack_timeout_ms: 30_000,
            dedupe_window_seconds: 86_400,
            max_message_bytes: MAX_REQUEST_BYTES,
            max_deliveries: 5,
        }
        .validate()
        .unwrap();

        let mut without_dedupe = request(DataOperation::Write);
        without_dedupe.dedupe_key = None;
        assert_eq!(
            without_dedupe.validate_for(WebApiMode::JetStreamAsync),
            Err(DataPlaneError::MissingDedupeKey)
        );

        let receipt = AsyncReceipt {
            operation_id: "op-01".to_string(),
            dedupe_key: "tenant-01:req-01".to_string(),
            status: AsyncStatus::Pending,
            attempt: 0,
            metadata: BTreeMap::new(),
        };
        let published = receipt.clone().transition(AsyncStatus::Published).unwrap();
        let processing = published.transition(AsyncStatus::Processing).unwrap();
        assert_eq!(receipt.status, AsyncStatus::Pending);
        assert_eq!(receipt.attempt, 0);
        assert_eq!(processing.attempt, 1);
        let succeeded = processing.transition(AsyncStatus::Succeeded).unwrap();
        assert!(receipt.transition(AsyncStatus::Processing).is_err());
        assert_eq!(succeeded.status, AsyncStatus::Succeeded);
    }
}
