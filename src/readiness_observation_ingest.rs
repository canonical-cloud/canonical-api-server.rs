//! Signed, append-only readiness observations reported by customer systems.
//!
//! Transport verification proves origin and exact-body integrity only. Every
//! accepted observation starts as `unreviewed`; it is not an audit opinion,
//! attestation, certification, authorization, or legal conclusion.

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fmt;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub const KEYS_ENV: &str = "CANONICAL_READINESS_INGEST_KEYS_JSON";
const EVENT_SPEC: &str = "canonical.readiness.observation.v1";
const RECEIPT_SPEC: &str = "canonical.readiness.observation.receipt.v1";
const PROBLEM_SPEC: &str = "canonical.readiness.observation.problem.v1";
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;
const MEMORY_CAPACITY: usize = 10_000;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct ObservationService {
    database: Option<DatabaseConnection>,
    keys: Arc<KeyRing>,
    memory: Arc<Mutex<MemoryStore>>,
}

impl fmt::Debug for ObservationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservationService")
            .field("database_configured", &self.database.is_some())
            .field("configured_keys", &self.keys.by_id.len())
            .finish()
    }
}

impl ObservationService {
    pub fn from_env(database: Option<DatabaseConnection>) -> Result<Self, BuildError> {
        let keys = match env::var(KEYS_ENV) {
            Ok(document) => KeyRing::parse(&document)?,
            Err(env::VarError::NotPresent) => KeyRing::default(),
            Err(env::VarError::NotUnicode(_)) => return Err(BuildError::NonUnicodeKeyDocument),
        };
        Ok(Self {
            database,
            keys: Arc::new(keys),
            memory: Arc::new(Mutex::new(MemoryStore::default())),
        })
    }

    #[cfg(test)]
    fn with_key_document(document: &str) -> Result<Self, BuildError> {
        Ok(Self {
            database: None,
            keys: Arc::new(KeyRing::parse(document)?),
            memory: Arc::new(Mutex::new(MemoryStore::default())),
        })
    }

    async fn ingest(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Result<ObservationReceipt, IngestError> {
        if body.is_empty() || body.len() > MAX_BODY_BYTES {
            return Err(IngestError::InvalidRequest);
        }
        let payload_sha256 = sha256_label(body);
        verify_content_digest(headers, body)?;
        let event: ObservationEvent =
            serde_json::from_slice(body).map_err(|_| IngestError::InvalidRequest)?;
        validate_event(&event, now)?;
        require_header(headers, "webhook-id", &event.event_id)?;
        require_header(headers, "idempotency-key", &event.event_id)?;
        let transport = self.keys.verify(headers, body, &event, now)?;

        let duplicate = match &self.database {
            Some(database) => {
                store_database(database, &transport, &event, &payload_sha256, body.len()).await?
            }
            None => self.store_memory(&transport, &event, &payload_sha256)?,
        };

        let received_at = now
            .format(&Rfc3339)
            .map_err(|_| IngestError::Internal)?;
        Ok(ObservationReceipt {
            spec_version: RECEIPT_SPEC,
            receipt_id: format!("rcpt_{}", Uuid::new_v4().simple()),
            event_id: event.event_id,
            source_id: event.source_id,
            source_sequence: event.source_sequence,
            status: if duplicate { "duplicate" } else { "accepted" },
            duplicate,
            payload_sha256,
            received_at,
            transport_verification: "signature-valid",
            substantive_review: "unreviewed",
        })
    }

    fn store_memory(
        &self,
        transport: &VerifiedTransport,
        event: &ObservationEvent,
        payload_sha256: &str,
    ) -> Result<bool, IngestError> {
        let mut memory = self
            .memory
            .lock()
            .map_err(|_| IngestError::StorageUnavailable)?;
        let event_key = (
            transport.owner_subject.clone(),
            event.source_id.clone(),
            event.event_id.clone(),
        );
        if let Some(existing) = memory.events.get(&event_key) {
            return if existing == payload_sha256 {
                Ok(true)
            } else {
                Err(IngestError::EventConflict)
            };
        }

        let stream_key = (transport.owner_subject.clone(), event.source_id.clone());
        if memory
            .sequences
            .get(&stream_key)
            .is_some_and(|sequence| event.source_sequence <= *sequence)
        {
            return Err(IngestError::SequenceConflict);
        }
        if memory.order.len() >= MEMORY_CAPACITY {
            return Err(IngestError::StorageUnavailable);
        }
        memory.events.insert(event_key.clone(), payload_sha256.into());
        memory
            .sequences
            .insert(stream_key, event.source_sequence);
        memory.order.push_back(event_key);
        Ok(false)
    }
}

pub fn router(service: ObservationService) -> Router {
    Router::new()
        .route("/api/v1/readiness/observations", post(ingest_observation))
        .route("/v1/readiness/observations", post(ingest_observation))
        .with_state(service)
}

async fn ingest_observation(
    State(service): State<ObservationService>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<ObservationReceipt>), IngestProblem> {
    let receipt = service
        .ingest(&headers, &body, OffsetDateTime::now_utc())
        .await
        .map_err(IngestProblem::from)?;
    let status = if receipt.duplicate {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(receipt)))
}

#[derive(Default)]
struct MemoryStore {
    events: HashMap<(String, String, String), String>,
    sequences: HashMap<(String, String), i64>,
    order: VecDeque<(String, String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyDocument {
    keys: Vec<KeyInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyInput {
    key_id: String,
    source_id: String,
    organization: String,
    owner_subject: String,
    secret: String,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_until: Option<String>,
}

#[derive(Default)]
struct KeyRing {
    by_id: HashMap<String, IngestKey>,
}

struct IngestKey {
    source_id: String,
    organization: String,
    owner_subject: String,
    secret: Vec<u8>,
    valid_from: Option<OffsetDateTime>,
    valid_until: Option<OffsetDateTime>,
}

impl KeyRing {
    fn parse(document: &str) -> Result<Self, BuildError> {
        let document: KeyDocument =
            serde_json::from_str(document).map_err(|_| BuildError::InvalidKeyDocument)?;
        if document.keys.len() > 128 {
            return Err(BuildError::TooManyKeys);
        }
        let mut by_id = HashMap::new();
        for input in document.keys {
            if !portable_id(&input.key_id)
                || !portable_id(&input.source_id)
                || input.organization.trim().is_empty()
                || input.organization.len() > 200
                || !valid_subject(&input.owner_subject)
            {
                return Err(BuildError::InvalidKeyDocument);
            }
            let encoded = input
                .secret
                .strip_prefix("whsec_")
                .ok_or(BuildError::InvalidSecret)?;
            let secret = decode_base64(encoded).map_err(|_| BuildError::InvalidSecret)?;
            if secret.len() < 24 {
                return Err(BuildError::InvalidSecret);
            }
            let valid_from = input
                .valid_from
                .as_deref()
                .map(parse_timestamp)
                .transpose()
                .map_err(|_| BuildError::InvalidKeyDocument)?;
            let valid_until = input
                .valid_until
                .as_deref()
                .map(parse_timestamp)
                .transpose()
                .map_err(|_| BuildError::InvalidKeyDocument)?;
            if valid_from
                .zip(valid_until)
                .is_some_and(|(start, end)| end <= start)
            {
                return Err(BuildError::InvalidKeyDocument);
            }
            if by_id
                .insert(
                    input.key_id,
                    IngestKey {
                        source_id: input.source_id,
                        organization: input.organization,
                        owner_subject: input.owner_subject,
                        secret,
                        valid_from,
                        valid_until,
                    },
                )
                .is_some()
            {
                return Err(BuildError::DuplicateKeyId);
            }
        }
        Ok(Self { by_id })
    }

    fn verify(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        event: &ObservationEvent,
        now: OffsetDateTime,
    ) -> Result<VerifiedTransport, IngestError> {
        let key_id = header(headers, "webhook-key-id")?;
        let key = self.by_id.get(key_id).ok_or(IngestError::UnknownSource)?;
        if key.source_id != event.source_id || key.organization != event.organization {
            return Err(IngestError::UnknownSource);
        }
        if key.valid_from.is_some_and(|start| now < start)
            || key.valid_until.is_some_and(|end| now >= end)
        {
            return Err(IngestError::UnknownSource);
        }
        let timestamp_text = header(headers, "webhook-timestamp")?;
        let timestamp = timestamp_text
            .parse::<i64>()
            .map_err(|_| IngestError::InvalidSignature)?;
        if now.unix_timestamp().abs_diff(timestamp) > MAX_CLOCK_SKEW_SECONDS as u64 {
            return Err(IngestError::StaleSignature);
        }
        let supplied = header(headers, "webhook-signature")?;
        let mut mac = HmacSha256::new_from_slice(&key.secret)
            .map_err(|_| IngestError::Internal)?;
        mac.update(event.event_id.as_bytes());
        mac.update(b".");
        mac.update(timestamp_text.as_bytes());
        mac.update(b".");
        mac.update(body);
        let expected = mac.finalize().into_bytes();
        let valid = supplied.split_whitespace().any(|candidate| {
            candidate
                .strip_prefix("v1,")
                .and_then(|encoded| decode_base64(encoded).ok())
                .is_some_and(|decoded| {
                    decoded.len() == expected.len()
                        && bool::from(decoded.as_slice().ct_eq(expected.as_slice()))
                })
        });
        if !valid {
            return Err(IngestError::InvalidSignature);
        }
        Ok(VerifiedTransport {
            key_id: key_id.to_owned(),
            owner_subject: key.owner_subject.clone(),
        })
    }
}

struct VerifiedTransport {
    key_id: String,
    owner_subject: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationEvent {
    spec_version: String,
    event_id: String,
    source_id: String,
    source_sequence: i64,
    organization: String,
    observed_at: String,
    assertions: Vec<ObservationAssertion>,
    evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationAssertion {
    framework_id: String,
    control_id: String,
    reported_status: ReportedStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    statement: Option<String>,
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

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObservationReceipt {
    spec_version: &'static str,
    receipt_id: String,
    event_id: String,
    source_id: String,
    source_sequence: i64,
    status: &'static str,
    duplicate: bool,
    payload_sha256: String,
    received_at: String,
    transport_verification: &'static str,
    substantive_review: &'static str,
}

fn validate_event(event: &ObservationEvent, now: OffsetDateTime) -> Result<(), IngestError> {
    if event.spec_version != EVENT_SPEC
        || !portable_id(&event.event_id)
        || !portable_id(&event.source_id)
        || event.source_sequence <= 0
        || event.organization.trim().is_empty()
        || event.organization.len() > 200
        || event.assertions.is_empty()
        || event.assertions.len() > 512
        || event.evidence.len() > 512
    {
        return Err(IngestError::InvalidRequest);
    }
    let observed_at = parse_timestamp(&event.observed_at).map_err(|_| IngestError::InvalidRequest)?;
    if observed_at.unix_timestamp() - now.unix_timestamp() > MAX_CLOCK_SKEW_SECONDS {
        return Err(IngestError::InvalidRequest);
    }

    let mut evidence_ids = HashSet::new();
    for evidence in &event.evidence {
        if !portable_id(&evidence.evidence_id)
            || evidence.kind.trim().is_empty()
            || evidence.kind.len() > 80
            || !valid_sha256(&evidence.sha256)
            || evidence.content_type.as_ref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > 160
                    || value.bytes().any(|byte| byte.is_ascii_control())
            })
            || evidence
                .size_bytes
                .is_some_and(|size| size > 1_099_511_627_776)
            || !evidence_ids.insert(evidence.evidence_id.as_str())
        {
            return Err(IngestError::InvalidRequest);
        }
        let collected =
            parse_timestamp(&evidence.collected_at).map_err(|_| IngestError::InvalidRequest)?;
        if collected.unix_timestamp() - now.unix_timestamp() > MAX_CLOCK_SKEW_SECONDS
            || collected.unix_timestamp() - observed_at.unix_timestamp()
                > MAX_CLOCK_SKEW_SECONDS
        {
            return Err(IngestError::InvalidRequest);
        }
        if let Some(locator) = &evidence.locator {
            validate_locator(locator)?;
        }
    }

    let mut assertions = HashSet::new();
    for assertion in &event.assertions {
        if !portable_name(&assertion.framework_id, 2, 128)
            || !portable_name(&assertion.control_id, 1, 128)
            || assertion
                .statement
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
            || assertion.evidence_ids.len() > 128
            || !assertions.insert((
                assertion.framework_id.as_str(),
                assertion.control_id.as_str(),
            ))
        {
            return Err(IngestError::InvalidRequest);
        }
        let mut references = HashSet::new();
        for evidence_id in &assertion.evidence_ids {
            if !references.insert(evidence_id.as_str())
                || !evidence_ids.contains(evidence_id.as_str())
            {
                return Err(IngestError::InvalidRequest);
            }
        }
    }
    Ok(())
}

fn validate_locator(locator: &str) -> Result<(), IngestError> {
    if locator.is_empty() || locator.len() > 2048 {
        return Err(IngestError::InvalidRequest);
    }
    let parsed = reqwest::Url::parse(locator).map_err(|_| IngestError::InvalidRequest)?;
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(IngestError::InvalidRequest);
    }
    match parsed.scheme() {
        "urn" => Ok(()),
        "https" if parsed.host_str().is_some() => Ok(()),
        _ => Err(IngestError::InvalidRequest),
    }
}

async fn store_database(
    database: &DatabaseConnection,
    transport: &VerifiedTransport,
    event: &ObservationEvent,
    payload_sha256: &str,
    body_len: usize,
) -> Result<bool, IngestError> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(IngestError::StorageUnavailable);
    }
    let transaction = database.begin().await?;
    set_subject(&transaction, &transport.owner_subject).await?;
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [format!(
                "readiness\\0{}\\0{}",
                transport.owner_subject, event.source_id
            )
            .into()],
        ))
        .await?;

    let existing = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT payload_sha256
            FROM canonical_readiness_observation
            WHERE owner_subject = $1
              AND source_id = $2
              AND event_id = $3
            "#,
            [
                transport.owner_subject.clone().into(),
                event.source_id.clone().into(),
                event.event_id.clone().into(),
            ],
        ))
        .await?;
    if let Some(row) = existing {
        let stored: String = row.try_get("", "payload_sha256")?;
        if stored == payload_sha256 {
            transaction.commit().await?;
            return Ok(true);
        }
        transaction.rollback().await?;
        return Err(IngestError::EventConflict);
    }

    let cursor = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT source_sequence
            FROM canonical_readiness_observation
            WHERE owner_subject = $1
              AND source_id = $2
            ORDER BY source_sequence DESC
            LIMIT 1
            "#,
            [
                transport.owner_subject.clone().into(),
                event.source_id.clone().into(),
            ],
        ))
        .await?;
    if cursor
        .map(|row| row.try_get::<i64>("", "source_sequence"))
        .transpose()?
        .is_some_and(|sequence| event.source_sequence <= sequence)
    {
        transaction.rollback().await?;
        return Err(IngestError::SequenceConflict);
    }

    let event_json: JsonValue =
        serde_json::to_value(event).map_err(|_| IngestError::Internal)?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_readiness_observation (
                owner_subject,
                source_id,
                event_id,
                source_sequence,
                organization,
                observed_at,
                payload_sha256,
                key_id,
                event_json,
                raw_body_octets,
                transport_verification,
                substantive_review
            )
            VALUES (
                $1, $2, $3, $4, $5, CAST($6 AS timestamptz), $7, $8, $9,
                $10, 'signature-valid', 'unreviewed'
            )
            "#,
            [
                transport.owner_subject.clone().into(),
                event.source_id.clone().into(),
                event.event_id.clone().into(),
                event.source_sequence.into(),
                event.organization.clone().into(),
                event.observed_at.clone().into(),
                payload_sha256.to_owned().into(),
                transport.key_id.clone().into(),
                event_json.into(),
                i64::try_from(body_len)
                    .map_err(|_| IngestError::InvalidRequest)?
                    .into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(false)
}

async fn set_subject(
    transaction: &DatabaseTransaction,
    subject: &str,
) -> Result<(), IngestError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.current_subject', $1, true)",
            [subject.to_owned().into()],
        ))
        .await?;
    Ok(())
}

fn verify_content_digest(headers: &HeaderMap, body: &[u8]) -> Result<(), IngestError> {
    let value = header(headers, "content-digest")?;
    let encoded = value
        .strip_prefix("sha-256=:")
        .and_then(|value| value.strip_suffix(':'))
        .ok_or(IngestError::InvalidDigest)?;
    let supplied = decode_base64(encoded).map_err(|_| IngestError::InvalidDigest)?;
    let expected = Sha256::digest(body);
    if supplied.len() != expected.len()
        || !bool::from(supplied.as_slice().ct_eq(expected.as_slice()))
    {
        return Err(IngestError::InvalidDigest);
    }
    Ok(())
}

fn header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, IngestError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or(IngestError::InvalidSignature)
}

fn require_header(
    headers: &HeaderMap,
    name: &'static str,
    expected: &str,
) -> Result<(), IngestError> {
    if header(headers, name)? == expected {
        Ok(())
    } else {
        Err(IngestError::InvalidSignature)
    }
}

fn portable_id(value: &str) -> bool {
    portable_name(value, 8, 128)
}

fn portable_name(value: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
}

fn valid_subject(value: &str) -> bool {
    portable_name(value, 1, 255)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(value, &Rfc3339)
}

fn sha256_label(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn decode_base64(input: &str) -> Result<Vec<u8>, ()> {
    if input.is_empty() || input.len() % 4 != 0 || !input.is_ascii() {
        return Err(());
    }
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let a = base64_value(chunk[0]).ok_or(())?;
        let b = base64_value(chunk[1]).ok_or(())?;
        let c_padding = chunk[2] == b'=';
        let d_padding = chunk[3] == b'=';
        if (!last && (c_padding || d_padding)) || (c_padding && !d_padding) {
            return Err(());
        }
        let c = if c_padding {
            0
        } else {
            base64_value(chunk[2]).ok_or(())?
        };
        let d = if d_padding {
            0
        } else {
            base64_value(chunk[3]).ok_or(())?
        };
        output.push((a << 2) | (b >> 4));
        if !c_padding {
            output.push((b << 4) | (c >> 2));
        }
        if !d_padding {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("readiness key document is invalid")]
    InvalidKeyDocument,
    #[error("readiness key document contains duplicate key ids")]
    DuplicateKeyId,
    #[error("readiness webhook secret is invalid")]
    InvalidSecret,
    #[error("readiness key document is not valid UTF-8")]
    NonUnicodeKeyDocument,
    #[error("readiness key document contains too many keys")]
    TooManyKeys,
}

#[derive(Debug, Error)]
enum IngestError {
    #[error("readiness observation request is invalid")]
    InvalidRequest,
    #[error("content digest is invalid")]
    InvalidDigest,
    #[error("webhook signature is invalid")]
    InvalidSignature,
    #[error("webhook timestamp is outside the replay window")]
    StaleSignature,
    #[error("readiness source is not configured")]
    UnknownSource,
    #[error("event id was reused with different content")]
    EventConflict,
    #[error("source sequence is not strictly increasing")]
    SequenceConflict,
    #[error("readiness observation storage is unavailable")]
    StorageUnavailable,
    #[error("internal readiness ingest error")]
    Internal,
    #[error("readiness observation database is unavailable")]
    Database(#[from] DbErr),
}

struct IngestProblem {
    status: StatusCode,
    problem: ObservationProblem,
}

impl From<IngestError> for IngestProblem {
    fn from(error: IngestError) -> Self {
        let (status, code, message) = match error {
            IngestError::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "readiness observation request is invalid",
            ),
            IngestError::InvalidDigest => (
                StatusCode::BAD_REQUEST,
                "invalid-digest",
                "content digest is invalid",
            ),
            IngestError::InvalidSignature => (
                StatusCode::UNAUTHORIZED,
                "invalid-signature",
                "webhook signature is invalid",
            ),
            IngestError::StaleSignature => (
                StatusCode::UNAUTHORIZED,
                "stale-signature",
                "webhook timestamp is outside the replay window",
            ),
            IngestError::UnknownSource => (
                StatusCode::UNAUTHORIZED,
                "unknown-source",
                "readiness source is not configured",
            ),
            IngestError::EventConflict => (
                StatusCode::CONFLICT,
                "event-conflict",
                "event id was reused with different content",
            ),
            IngestError::SequenceConflict => (
                StatusCode::CONFLICT,
                "sequence-conflict",
                "source sequence is not strictly increasing",
            ),
            IngestError::StorageUnavailable | IngestError::Database(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "storage-unavailable",
                "readiness observation storage is unavailable",
            ),
            IngestError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "readiness observation could not be processed",
            ),
        };
        Self {
            status,
            problem: ObservationProblem {
                spec_version: PROBLEM_SPEC,
                code,
                message,
                request_id: Uuid::new_v4().to_string(),
            },
        }
    }
}

impl IntoResponse for IngestProblem {
    fn into_response(self) -> Response {
        (self.status, Json(self.problem)).into_response()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ObservationProblem {
    spec_version: &'static str,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET_BYTES: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn encode_base64(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let a = chunk[0];
            let b = *chunk.get(1).unwrap_or(&0);
            let c = *chunk.get(2).unwrap_or(&0);
            output.push(ALPHABET[(a >> 2) as usize] as char);
            output.push(ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
            if chunk.len() > 1 {
                output.push(ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
            } else {
                output.push('=');
            }
            if chunk.len() > 2 {
                output.push(ALPHABET[(c & 0x3f) as usize] as char);
            } else {
                output.push('=');
            }
        }
        output
    }

    fn event(sequence: i64) -> ObservationEvent {
        ObservationEvent {
            spec_version: EVENT_SPEC.into(),
            event_id: format!("event.test.{sequence:08}"),
            source_id: "source.customer-ci".into(),
            source_sequence: sequence,
            organization: "Example Incorporated".into(),
            observed_at: "2026-09-09T05:00:00Z".into(),
            assertions: vec![ObservationAssertion {
                framework_id: "soc2-tsc".into(),
                control_id: "CC6.1".into(),
                reported_status: ReportedStatus::Observed,
                statement: None,
                evidence_ids: vec!["evidence.access-review".into()],
            }],
            evidence: vec![EvidenceItem {
                evidence_id: "evidence.access-review".into(),
                kind: "configuration-snapshot".into(),
                sha256: format!("sha256:{}", "a".repeat(64)),
                collected_at: "2026-09-09T04:59:00Z".into(),
                locator: Some("urn:customer-evidence:access-review:42".into()),
                content_type: Some("application/json".into()),
                size_bytes: Some(2048),
            }],
        }
    }

    fn service() -> ObservationService {
        let document = format!(
            r#"{{"keys":[{{"keyId":"key.customer-ci","sourceId":"source.customer-ci","organization":"Example Incorporated","ownerSubject":"org:example","secret":"whsec_{}"}}]}}"#,
            encode_base64(SECRET_BYTES)
        );
        ObservationService::with_key_document(&document).unwrap()
    }

    fn signed_headers(body: &[u8], event_id: &str, timestamp: i64) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-digest",
            format!("sha-256=:{}:", encode_base64(&Sha256::digest(body)))
                .parse()
                .unwrap(),
        );
        headers.insert("webhook-id", event_id.parse().unwrap());
        headers.insert("idempotency-key", event_id.parse().unwrap());
        headers.insert("webhook-key-id", "key.customer-ci".parse().unwrap());
        headers.insert("webhook-timestamp", timestamp.to_string().parse().unwrap());
        let mut mac = HmacSha256::new_from_slice(SECRET_BYTES).unwrap();
        mac.update(event_id.as_bytes());
        mac.update(b".");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        headers.insert(
            "webhook-signature",
            format!("v1,{}", encode_base64(&mac.finalize().into_bytes()))
                .parse()
                .unwrap(),
        );
        headers
    }

    #[test]
    fn base64_decoder_rejects_malformed_padding() {
        assert_eq!(decode_base64("YQ==").unwrap(), b"a");
        assert!(decode_base64("Y=Q=").is_err());
        assert!(decode_base64("YQ=").is_err());
    }

    #[test]
    fn semantic_validation_requires_referenced_evidence() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let mut candidate = event(1);
        candidate.assertions[0].evidence_ids = vec!["evidence.missing".into()];
        assert!(matches!(
            validate_event(&candidate, now),
            Err(IngestError::InvalidRequest)
        ));
    }

    #[test]
    fn locators_cannot_smuggle_credentials_or_query_tokens() {
        assert!(validate_locator("https://evidence.example/object").is_ok());
        assert!(validate_locator("urn:customer-evidence:access-review:42").is_ok());
        assert!(validate_locator("https://user:pass@example.com/object").is_err());
        assert!(validate_locator("https://example.com/object?token=secret").is_err());
    }

    #[tokio::test]
    async fn exact_replay_is_duplicate_but_event_mutation_conflicts() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let candidate = event(1);
        let body = serde_json::to_vec(&candidate).unwrap();
        let headers = signed_headers(&body, &candidate.event_id, now.unix_timestamp());
        let first = service().ingest(&headers, &body, now).await.unwrap();
        assert!(!first.duplicate);

        let service = service();
        let first = service.ingest(&headers, &body, now).await.unwrap();
        let second = service.ingest(&headers, &body, now).await.unwrap();
        assert!(!first.duplicate);
        assert!(second.duplicate);

        let mut altered = candidate;
        altered.assertions[0].reported_status = ReportedStatus::Unknown;
        let altered_body = serde_json::to_vec(&altered).unwrap();
        let altered_headers =
            signed_headers(&altered_body, &altered.event_id, now.unix_timestamp());
        assert!(matches!(
            service.ingest(&altered_headers, &altered_body, now).await,
            Err(IngestError::EventConflict)
        ));
    }

    #[tokio::test]
    async fn source_sequence_must_increase() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let service = service();
        let first = event(2);
        let first_body = serde_json::to_vec(&first).unwrap();
        let first_headers = signed_headers(&first_body, &first.event_id, now.unix_timestamp());
        service.ingest(&first_headers, &first_body, now).await.unwrap();

        let stale = event(1);
        let stale_body = serde_json::to_vec(&stale).unwrap();
        let stale_headers = signed_headers(&stale_body, &stale.event_id, now.unix_timestamp());
        assert!(matches!(
            service.ingest(&stale_headers, &stale_body, now).await,
            Err(IngestError::SequenceConflict)
        ));
    }

    #[tokio::test]
    async fn stale_signature_is_rejected_before_storage() {
        let now = parse_timestamp("2026-09-09T05:10:01Z").unwrap();
        let candidate = event(1);
        let body = serde_json::to_vec(&candidate).unwrap();
        let headers = signed_headers(&body, &candidate.event_id, 1_788_927_600);
        assert!(matches!(
            service().ingest(&headers, &body, now).await,
            Err(IngestError::StaleSignature)
        ));
    }
}
