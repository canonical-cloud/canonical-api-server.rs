//! Signed, source-bound ingestion for continuous-readiness observations.
//!
//! Transport authentication and syntactic validation do not establish that a
//! customer assertion is true, complete, representative, accepted as audit
//! evidence, compliant, attested, certified, or legally sufficient.

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fmt;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::error;
use uuid::Uuid;

const CONTRACT_VERSION: &str = "canonical.readiness.observation.v1";
const RECEIPT_VERSION: &str = "canonical.readiness.observation.receipt.v1";
pub const KEYRING_ENV: &str = "CANONICAL_READINESS_INGEST_KEYS_JSON";
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_KEYRING_BYTES: usize = 64 * 1024;
const MAX_KEYS: usize = 64;
const MAX_KEYS_PER_SOURCE: usize = 4;
const MAX_ASSERTIONS: usize = 256;
const MAX_EVIDENCE: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_STATEMENT_BYTES: usize = 2_048;
const MAX_LOCATOR_BYTES: usize = 2_048;
const MAX_CONTENT_TYPE_BYTES: usize = 128;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;
const MEMORY_CAPACITY: usize = 4_096;
const WEBHOOK_ID: &str = "x-canonical-webhook-id";
const WEBHOOK_TIMESTAMP: &str = "x-canonical-webhook-timestamp";
const WEBHOOK_SIGNATURE: &str = "x-canonical-webhook-signature";
const WEBHOOK_KEY_ID: &str = "x-canonical-webhook-key-id";
const CONTENT_DIGEST: &str = "content-digest";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct IngressState {
    database: Option<DatabaseConnection>,
    keyring: Keyring,
    memory: Arc<Mutex<VecDeque<StoredObservation>>>,
}

#[derive(Clone, Debug)]
struct StoredObservation {
    event_id: String,
    payload_sha256: String,
    received_at: String,
    receipt_id: String,
    sequence: u64,
    source_id: String,
    subject: String,
}

#[derive(Clone)]
struct Keyring {
    by_source: Arc<HashMap<String, Vec<IngressKey>>>,
}

impl fmt::Debug for Keyring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keyring")
            .field("sources", &self.by_source.len())
            .field(
                "keys",
                &self.by_source.values().map(Vec::len).sum::<usize>(),
            )
            .field("secret_material", &"[redacted]")
            .finish()
    }
}

#[derive(Clone)]
struct IngressKey {
    key_id: String,
    not_after: Option<OffsetDateTime>,
    not_before: Option<OffsetDateTime>,
    organization: String,
    secret: Arc<[u8]>,
    subject: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyringDocument {
    keys: Vec<IngressKeyInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IngressKeyInput {
    key_id: String,
    source_id: String,
    organization: String,
    subject: String,
    secret: String,
    #[serde(default)]
    not_before: Option<String>,
    #[serde(default)]
    not_after: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationEvent {
    spec_version: String,
    event_id: String,
    source_id: String,
    source_sequence: u64,
    organization: String,
    observed_at: String,
    assertions: Vec<ObservationAssertion>,
    evidence: Vec<EvidenceItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationAssertion {
    framework_id: String,
    control_id: String,
    reported_status: ReportedStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    statement: Option<String>,
    #[serde(default)]
    evidence_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReportedStatus {
    Observed,
    NotObserved,
    Exception,
    NotApplicableRequested,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceItem {
    evidence_id: String,
    kind: String,
    sha256: String,
    collected_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReceiptStatus {
    Accepted,
    Duplicate,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TransportVerification {
    SignatureValid,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SubstantiveReview {
    Unreviewed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObservationReceipt {
    spec_version: &'static str,
    receipt_id: String,
    event_id: String,
    source_id: String,
    source_sequence: u64,
    status: ReceiptStatus,
    duplicate: bool,
    payload_sha256: String,
    received_at: String,
    transport_verification: TransportVerification,
    substantive_review: SubstantiveReview,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObservationProblem {
    spec_version: &'static str,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

#[derive(Clone, Debug)]
struct StoreOutcome {
    duplicate: bool,
    received_at: String,
    receipt_id: String,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("{KEYRING_ENV} exceeds the 64 KiB configuration bound")]
    KeyringTooLarge,
    #[error("{KEYRING_ENV} is not a valid bounded keyring document")]
    InvalidKeyring,
}

#[derive(Debug, Error)]
enum IngestError {
    #[error("invalid request")]
    InvalidRequest,
    #[error("content digest is invalid")]
    InvalidDigest,
    #[error("signature is invalid")]
    InvalidSignature,
    #[error("signature timestamp is stale")]
    StaleSignature,
    #[error("source is not configured")]
    UnknownSource,
    #[error("event id conflicts with stored content")]
    EventConflict,
    #[error("source sequence conflicts with stored content")]
    SequenceConflict,
    #[error("storage is unavailable")]
    StorageUnavailable,
    #[error("internal error")]
    Internal,
}

impl IntoResponse for IngestError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "readiness observation request is invalid",
            ),
            Self::InvalidDigest => (
                StatusCode::BAD_REQUEST,
                "invalid-digest",
                "content digest does not match the exact request body",
            ),
            Self::InvalidSignature => (
                StatusCode::UNAUTHORIZED,
                "invalid-signature",
                "readiness observation authentication failed",
            ),
            Self::StaleSignature => (
                StatusCode::UNAUTHORIZED,
                "stale-signature",
                "readiness observation authentication failed",
            ),
            Self::UnknownSource => (
                StatusCode::UNAUTHORIZED,
                "unknown-source",
                "readiness observation authentication failed",
            ),
            Self::EventConflict => (
                StatusCode::CONFLICT,
                "event-conflict",
                "event id was already used for different content",
            ),
            Self::SequenceConflict => (
                StatusCode::CONFLICT,
                "sequence-conflict",
                "source sequence is not the next accepted observation",
            ),
            Self::StorageUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "storage-unavailable",
                "readiness observation storage is temporarily unavailable",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "readiness observation could not be processed",
            ),
        };
        let mut response = (
            status,
            Json(ObservationProblem {
                spec_version: "canonical.readiness.observation.problem.v1",
                code,
                message,
                request_id: Uuid::new_v4().to_string(),
            }),
        )
            .into_response();
        apply_response_headers(&mut response);
        response
    }
}

pub fn router(database: Option<DatabaseConnection>) -> Result<Router, BuildError> {
    let state = IngressState {
        database,
        keyring: Keyring::from_env()?,
        memory: Arc::new(Mutex::new(VecDeque::new())),
    };
    Ok(Router::new()
        .route(
            "/api/v1/readiness/observations",
            post(ingest_observation),
        )
        .route("/v1/readiness/observations", post(ingest_observation))
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)))
}

async fn ingest_observation(
    State(state): State<IngressState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, IngestError> {
    if body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(IngestError::InvalidRequest);
    }
    verify_content_digest(&headers, &body)?;
    let event = serde_json::from_slice::<ObservationEvent>(&body)
        .map_err(|_| IngestError::InvalidRequest)?;
    let now = OffsetDateTime::now_utc();
    validate_event(&event, now)?;
    let subject = state.keyring.authenticate(&event, &headers, &body, now)?;
    let payload_sha256 = payload_digest(&body);
    let outcome = persist(&state, &subject, &event, &payload_sha256, now).await?;
    let duplicate = outcome.duplicate;
    let status = if duplicate {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let mut response = (
        status,
        Json(ObservationReceipt {
            spec_version: RECEIPT_VERSION,
            receipt_id: outcome.receipt_id,
            event_id: event.event_id,
            source_id: event.source_id,
            source_sequence: event.source_sequence,
            status: if duplicate {
                ReceiptStatus::Duplicate
            } else {
                ReceiptStatus::Accepted
            },
            duplicate,
            payload_sha256,
            received_at: outcome.received_at,
            transport_verification: TransportVerification::SignatureValid,
            substantive_review: SubstantiveReview::Unreviewed,
        }),
    )
        .into_response();
    apply_response_headers(&mut response);
    Ok(response)
}

fn apply_response_headers(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
}

impl Keyring {
    fn from_env() -> Result<Self, BuildError> {
        let document = match env::var(KEYRING_ENV) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) | Err(env::VarError::NotPresent) => {
                return Ok(Self {
                    by_source: Arc::new(HashMap::new()),
                });
            }
            Err(env::VarError::NotUnicode(_)) => return Err(BuildError::InvalidKeyring),
        };
        Self::from_json(&document)
    }

    fn from_json(document: &str) -> Result<Self, BuildError> {
        if document.len() > MAX_KEYRING_BYTES {
            return Err(BuildError::KeyringTooLarge);
        }
        let document = serde_json::from_str::<KeyringDocument>(document)
            .map_err(|_| BuildError::InvalidKeyring)?;
        if document.keys.is_empty() || document.keys.len() > MAX_KEYS {
            return Err(BuildError::InvalidKeyring);
        }

        let mut key_ids = HashSet::new();
        let mut by_source: HashMap<String, Vec<IngressKey>> = HashMap::new();
        for input in document.keys {
            validate_identifier(&input.key_id).map_err(|_| BuildError::InvalidKeyring)?;
            validate_identifier(&input.source_id).map_err(|_| BuildError::InvalidKeyring)?;
            validate_identifier(&input.organization).map_err(|_| BuildError::InvalidKeyring)?;
            validate_identifier(&input.subject).map_err(|_| BuildError::InvalidKeyring)?;
            if !key_ids.insert(input.key_id.clone())
                || input
                    .secret
                    .bytes()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .count()
                    < 32
            {
                return Err(BuildError::InvalidKeyring);
            }
            let not_before = input
                .not_before
                .as_deref()
                .map(parse_rfc3339)
                .transpose()
                .map_err(|_| BuildError::InvalidKeyring)?;
            let not_after = input
                .not_after
                .as_deref()
                .map(parse_rfc3339)
                .transpose()
                .map_err(|_| BuildError::InvalidKeyring)?;
            if matches!((not_before, not_after), (Some(start), Some(end)) if start >= end) {
                return Err(BuildError::InvalidKeyring);
            }
            let keys = by_source.entry(input.source_id).or_default();
            if keys.len() >= MAX_KEYS_PER_SOURCE {
                return Err(BuildError::InvalidKeyring);
            }
            keys.push(IngressKey {
                key_id: input.key_id,
                not_after,
                not_before,
                organization: input.organization,
                secret: Arc::from(input.secret.into_bytes()),
                subject: input.subject,
            });
        }
        Ok(Self {
            by_source: Arc::new(by_source),
        })
    }

    fn authenticate(
        &self,
        event: &ObservationEvent,
        headers: &HeaderMap,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Result<String, IngestError> {
        let timestamp = required_header(headers, WEBHOOK_TIMESTAMP)?
            .parse::<i64>()
            .map_err(|_| IngestError::InvalidSignature)?;
        if timestamp < now.unix_timestamp().saturating_sub(MAX_CLOCK_SKEW_SECONDS)
            || timestamp > now.unix_timestamp().saturating_add(MAX_CLOCK_SKEW_SECONDS)
        {
            return Err(IngestError::StaleSignature);
        }
        if required_header(headers, WEBHOOK_ID)? != event.event_id {
            return Err(IngestError::InvalidSignature);
        }
        let supplied = required_header(headers, WEBHOOK_SIGNATURE)?;
        let requested_key = optional_header(headers, WEBHOOK_KEY_ID)?;
        let keys = self
            .by_source
            .get(&event.source_id)
            .ok_or(IngestError::UnknownSource)?;

        for key in keys {
            if requested_key.is_some_and(|key_id| key_id != key.key_id)
                || key.organization != event.organization
                || key.not_before.is_some_and(|start| now < start)
                || key.not_after.is_some_and(|end| now >= end)
            {
                continue;
            }
            if verify_signature(&key.secret, timestamp, body, supplied).is_ok() {
                return Ok(key.subject.clone());
            }
        }
        Err(IngestError::InvalidSignature)
    }
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, IngestError> {
    optional_header(headers, name)?.ok_or(IngestError::InvalidSignature)
}

fn optional_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, IngestError> {
    headers
        .get(name)
        .map(|value| value.to_str().map_err(|_| IngestError::InvalidSignature))
        .transpose()
}

fn verify_content_digest(headers: &HeaderMap, body: &[u8]) -> Result<(), IngestError> {
    let Some(value) = headers.get(CONTENT_DIGEST) else {
        return Ok(());
    };
    let value = value.to_str().map_err(|_| IngestError::InvalidDigest)?;
    if value != payload_digest(body) {
        return Err(IngestError::InvalidDigest);
    }
    Ok(())
}

fn verify_signature(
    secret: &[u8],
    timestamp: i64,
    body: &[u8],
    supplied: &str,
) -> Result<(), IngestError> {
    let encoded = supplied
        .strip_prefix("v1=")
        .filter(|value| value.len() == 64 && value.bytes().all(is_lower_hex))
        .ok_or(IngestError::InvalidSignature)?;
    let signature = decode_hex_32(encoded).ok_or(IngestError::InvalidSignature)?;
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| IngestError::Internal)?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| IngestError::InvalidSignature)
}

fn validate_event(event: &ObservationEvent, now: OffsetDateTime) -> Result<(), IngestError> {
    if event.spec_version != CONTRACT_VERSION
        || event.source_sequence == 0
        || event.assertions.is_empty()
        || event.assertions.len() > MAX_ASSERTIONS
        || event.evidence.len() > MAX_EVIDENCE
    {
        return Err(IngestError::InvalidRequest);
    }
    for value in [&event.event_id, &event.source_id, &event.organization] {
        validate_identifier(value)?;
    }
    let observed_at = parse_rfc3339(&event.observed_at)?;
    if observed_at > now + time::Duration::seconds(MAX_CLOCK_SKEW_SECONDS) {
        return Err(IngestError::InvalidRequest);
    }

    let mut evidence_ids = HashSet::with_capacity(event.evidence.len());
    for evidence in &event.evidence {
        validate_identifier(&evidence.evidence_id)?;
        validate_bounded_text(&evidence.kind, 1, 64)?;
        if !valid_sha256(&evidence.sha256)
            || parse_rfc3339(&evidence.collected_at)?
                > now + time::Duration::seconds(MAX_CLOCK_SKEW_SECONDS)
        {
            return Err(IngestError::InvalidRequest);
        }
        if let Some(locator) = &evidence.locator {
            validate_bounded_text(locator, 1, MAX_LOCATOR_BYTES)?;
            if !(locator.starts_with("https://") || locator.starts_with("urn:")) {
                return Err(IngestError::InvalidRequest);
            }
        }
        if let Some(content_type) = &evidence.content_type {
            validate_bounded_text(content_type, 1, MAX_CONTENT_TYPE_BYTES)?;
            if content_type.contains('\r') || content_type.contains('\n') {
                return Err(IngestError::InvalidRequest);
            }
        }
        if evidence.size_bytes.is_some_and(|size| size > (1_u64 << 50))
            || !evidence_ids.insert(evidence.evidence_id.as_str())
        {
            return Err(IngestError::InvalidRequest);
        }
    }

    let mut assertion_keys = HashSet::with_capacity(event.assertions.len());
    for assertion in &event.assertions {
        validate_identifier(&assertion.framework_id)?;
        validate_identifier(&assertion.control_id)?;
        let _status = assertion.reported_status;
        if let Some(statement) = &assertion.statement {
            validate_bounded_text(statement, 1, MAX_STATEMENT_BYTES)?;
        }
        if !assertion_keys.insert((
            assertion.framework_id.as_str(),
            assertion.control_id.as_str(),
        )) {
            return Err(IngestError::InvalidRequest);
        }
        let mut referenced = HashSet::new();
        for evidence_id in &assertion.evidence_ids {
            validate_identifier(evidence_id)?;
            if !referenced.insert(evidence_id)
                || !evidence_ids.contains(evidence_id.as_str())
            {
                return Err(IngestError::InvalidRequest);
            }
        }
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), IngestError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(IngestError::InvalidRequest);
    }
    Ok(())
}

fn validate_bounded_text(value: &str, minimum: usize, maximum: usize) -> Result<(), IngestError> {
    if value.len() < minimum
        || value.len() > maximum
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(IngestError::InvalidRequest);
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(is_lower_hex))
}

fn parse_rfc3339(value: &str) -> Result<OffsetDateTime, IngestError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| IngestError::InvalidRequest)
}

fn format_rfc3339(value: OffsetDateTime) -> Result<String, IngestError> {
    value.format(&Rfc3339).map_err(|_| IngestError::Internal)
}

fn is_lower_hex(value: u8) -> bool {
    value.is_ascii_digit() || (b'a'..=b'f').contains(&value)
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let bytes = value.as_bytes();
    let mut output = [0_u8; 32];
    for (index, destination) in output.iter_mut().enumerate() {
        let high = decode_nibble(bytes[index * 2])?;
        let low = decode_nibble(bytes[index * 2 + 1])?;
        *destination = (high << 4) | low;
    }
    Some(output)
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn payload_digest(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut output = String::with_capacity(7 + digest.len() * 2);
    output.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

async fn persist(
    state: &IngressState,
    subject: &str,
    event: &ObservationEvent,
    payload_sha256: &str,
    now: OffsetDateTime,
) -> Result<StoreOutcome, IngestError> {
    if let Some(database) = &state.database {
        return persist_database(database, subject, event, payload_sha256).await;
    }
    persist_memory(state, subject, event, payload_sha256, now)
}

fn persist_memory(
    state: &IngressState,
    subject: &str,
    event: &ObservationEvent,
    payload_sha256: &str,
    now: OffsetDateTime,
) -> Result<StoreOutcome, IngestError> {
    let mut entries = state.memory.lock().map_err(|_| IngestError::Internal)?;
    if let Some(existing) = entries
        .iter()
        .find(|entry| entry.subject == subject && entry.event_id == event.event_id)
    {
        if existing.payload_sha256 == payload_sha256
            && existing.source_id == event.source_id
            && existing.sequence == event.source_sequence
        {
            return Ok(StoreOutcome {
                duplicate: true,
                received_at: existing.received_at.clone(),
                receipt_id: existing.receipt_id.clone(),
            });
        }
        return Err(IngestError::EventConflict);
    }
    if entries.iter().any(|entry| {
        entry.subject == subject
            && entry.source_id == event.source_id
            && entry.sequence >= event.source_sequence
    }) {
        return Err(IngestError::SequenceConflict);
    }

    let received_at = format_rfc3339(now)?;
    let receipt_id = Uuid::new_v4().to_string();
    entries.push_back(StoredObservation {
        event_id: event.event_id.clone(),
        payload_sha256: payload_sha256.to_owned(),
        received_at: received_at.clone(),
        receipt_id: receipt_id.clone(),
        sequence: event.source_sequence,
        source_id: event.source_id.clone(),
        subject: subject.to_owned(),
    });
    if entries.len() > MEMORY_CAPACITY {
        entries.pop_front();
    }
    Ok(StoreOutcome {
        duplicate: false,
        received_at,
        receipt_id,
    })
}

async fn persist_database(
    database: &DatabaseConnection,
    subject: &str,
    event: &ObservationEvent,
    payload_sha256: &str,
) -> Result<StoreOutcome, IngestError> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(IngestError::StorageUnavailable);
    }
    let transaction = database.begin().await.map_err(store_error)?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.current_subject', $1, true)",
            [subject.to_owned().into()],
        ))
        .await
        .map_err(store_error)?;
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [format!("readiness-observation\0{subject}\0{}", event.source_id).into()],
        ))
        .await
        .map_err(store_error)?;

    if let Some(row) = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT receipt_id, source_id, source_sequence, payload_sha256,
                   to_char(received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                       AS received_at_text
            FROM canonical_cloud__readiness.observation
            WHERE owner_subject = $1 AND event_id = $2
            "#,
            [subject.to_owned().into(), event.event_id.clone().into()],
        ))
        .await
        .map_err(store_error)?
    {
        let outcome = existing_event_outcome(&row, event, payload_sha256)?;
        transaction.commit().await.map_err(store_error)?;
        return Ok(outcome);
    }

    if let Some(row) = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT event_id, receipt_id, payload_sha256,
                   to_char(received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                       AS received_at_text
            FROM canonical_cloud__readiness.observation
            WHERE owner_subject = $1 AND source_id = $2 AND source_sequence = $3
            "#,
            [
                subject.to_owned().into(),
                event.source_id.clone().into(),
                i64::try_from(event.source_sequence)
                    .map_err(|_| IngestError::InvalidRequest)?
                    .into(),
            ],
        ))
        .await
        .map_err(store_error)?
    {
        let existing_event_id: String = row.try_get("", "event_id").map_err(store_error)?;
        let existing_digest: String = row
            .try_get("", "payload_sha256")
            .map_err(store_error)?;
        if existing_event_id == event.event_id && existing_digest == payload_sha256 {
            let outcome = row_outcome(&row, true)?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(outcome);
        }
        transaction.rollback().await.map_err(store_error)?;
        return Err(IngestError::SequenceConflict);
    }

    let max_sequence = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT MAX(source_sequence) AS max_sequence
            FROM canonical_cloud__readiness.observation
            WHERE owner_subject = $1 AND source_id = $2
            "#,
            [subject.to_owned().into(), event.source_id.clone().into()],
        ))
        .await
        .map_err(store_error)?
        .and_then(|row| row.try_get::<Option<i64>>("", "max_sequence").ok())
        .flatten();
    if max_sequence.is_some_and(|sequence| {
        i64::try_from(event.source_sequence).map_or(true, |incoming| incoming <= sequence)
    }) {
        transaction.rollback().await.map_err(store_error)?;
        return Err(IngestError::SequenceConflict);
    }

    let receipt_id = Uuid::new_v4();
    let event_json = serde_json::to_value(event).map_err(|_| IngestError::Internal)?;
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_cloud__readiness.observation (
                receipt_id, event_id, owner_subject, source_id, source_sequence,
                organization, payload_sha256, event_json, observed_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::timestamptz)
            RETURNING receipt_id,
                      to_char(received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                          AS received_at_text
            "#,
            [
                receipt_id.into(),
                event.event_id.clone().into(),
                subject.to_owned().into(),
                event.source_id.clone().into(),
                i64::try_from(event.source_sequence)
                    .map_err(|_| IngestError::InvalidRequest)?
                    .into(),
                event.organization.clone().into(),
                payload_sha256.to_owned().into(),
                event_json.into(),
                event.observed_at.clone().into(),
            ],
        ))
        .await
        .map_err(store_error)?
        .ok_or(IngestError::StorageUnavailable)?;
    let outcome = row_outcome(&row, false)?;
    transaction.commit().await.map_err(store_error)?;
    Ok(outcome)
}

fn existing_event_outcome(
    row: &QueryResult,
    event: &ObservationEvent,
    payload_sha256: &str,
) -> Result<StoreOutcome, IngestError> {
    let source_id: String = row.try_get("", "source_id").map_err(store_error)?;
    let sequence: i64 = row
        .try_get("", "source_sequence")
        .map_err(store_error)?;
    let digest: String = row
        .try_get("", "payload_sha256")
        .map_err(store_error)?;
    if source_id != event.source_id
        || u64::try_from(sequence).ok() != Some(event.source_sequence)
        || digest != payload_sha256
    {
        return Err(IngestError::EventConflict);
    }
    row_outcome(row, true)
}

fn row_outcome(row: &QueryResult, duplicate: bool) -> Result<StoreOutcome, IngestError> {
    let receipt_id: Uuid = row.try_get("", "receipt_id").map_err(store_error)?;
    let received_at: String = row
        .try_get("", "received_at_text")
        .map_err(store_error)?;
    Ok(StoreOutcome {
        duplicate,
        received_at,
        receipt_id: receipt_id.to_string(),
    })
}

fn store_error(error: DbErr) -> IngestError {
    error!(error_code = "readiness_observation_storage", %error, "readiness observation storage operation failed");
    IngestError::StorageUnavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const BODY: &[u8] = br#"{
      "specVersion":"canonical.readiness.observation.v1",
      "eventId":"evt_test_001",
      "sourceId":"customer-ci",
      "sourceSequence":1,
      "organization":"org:customer",
      "observedAt":"2026-09-08T23:00:00Z",
      "assertions":[{
        "frameworkId":"soc2-tsc",
        "controlId":"CC6.1",
        "reportedStatus":"observed",
        "evidenceIds":["ev_test_001"]
      }],
      "evidence":[{
        "evidenceId":"ev_test_001",
        "kind":"configuration-snapshot",
        "sha256":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "collectedAt":"2026-09-08T22:59:00Z",
        "locator":"urn:customer-evidence:test"
      }]
    }"#;

    fn event() -> ObservationEvent {
        serde_json::from_slice(BODY).expect("fixture should deserialize")
    }

    fn keyring() -> Keyring {
        Keyring::from_json(&format!(
            r#"{{"keys":[{{"keyId":"key-2026-09","sourceId":"customer-ci","organization":"org:customer","subject":"subject:customer","secret":"{SECRET}"}}]}}"#
        ))
        .expect("test keyring")
    }

    fn signed_headers(timestamp: i64) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(SECRET.as_bytes()).expect("HMAC key");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(BODY);
        let signature = mac.finalize().into_bytes();
        let mut encoded = String::from("v1=");
        for byte in signature {
            use fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("String write");
        }
        let mut headers = HeaderMap::new();
        headers.insert(WEBHOOK_ID, HeaderValue::from_static("evt_test_001"));
        headers.insert(
            WEBHOOK_TIMESTAMP,
            HeaderValue::from_str(&timestamp.to_string()).expect("timestamp header"),
        );
        headers.insert(
            WEBHOOK_SIGNATURE,
            HeaderValue::from_str(&encoded).expect("signature header"),
        );
        headers
    }

    #[test]
    fn keyring_debug_never_discloses_secret_material() {
        let rendered = format!("{:?}", keyring());
        assert!(!rendered.contains(SECRET));
        assert!(rendered.contains("[redacted]"));
    }

    #[test]
    fn exact_body_signature_binds_source_and_body() {
        let now = OffsetDateTime::from_unix_timestamp(1_788_908_400).expect("test time");
        let headers = signed_headers(now.unix_timestamp());
        assert_eq!(
            keyring()
                .authenticate(&event(), &headers, BODY, now)
                .expect("signature should verify"),
            "subject:customer"
        );
        assert!(matches!(
            keyring().authenticate(&event(), &headers, b"{}", now),
            Err(IngestError::InvalidSignature)
        ));
    }

    #[test]
    fn unknown_evidence_reference_is_rejected() {
        let mut event = event();
        event.assertions[0].evidence_ids[0] = "ev_missing".into();
        let now = OffsetDateTime::from_unix_timestamp(1_788_908_400).expect("test time");
        assert!(matches!(
            validate_event(&event, now),
            Err(IngestError::InvalidRequest)
        ));
    }

    #[test]
    fn memory_store_is_idempotent_and_rejects_conflicts() {
        let state = IngressState {
            database: None,
            keyring: keyring(),
            memory: Arc::new(Mutex::new(VecDeque::new())),
        };
        let event = event();
        let now = OffsetDateTime::from_unix_timestamp(1_788_908_400).expect("test time");
        let digest = payload_digest(BODY);
        let first = persist_memory(&state, "subject:customer", &event, &digest, now)
            .expect("first insert");
        assert!(!first.duplicate);
        let duplicate = persist_memory(&state, "subject:customer", &event, &digest, now)
            .expect("duplicate");
        assert!(duplicate.duplicate);

        let mut conflicting = event.clone();
        conflicting.organization = "org:changed".into();
        assert!(matches!(
            persist_memory(
                &state,
                "subject:customer",
                &conflicting,
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                now,
            ),
            Err(IngestError::EventConflict)
        ));
    }
}
