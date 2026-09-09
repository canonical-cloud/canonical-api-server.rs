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
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
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
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub const KEYS_ENV: &str = "CANONICAL_READINESS_INGEST_KEYS_JSON";
const EVENT_SPEC: &str = "canonical.readiness.observation.v1";
const RECEIPT_SPEC: &str = "canonical.readiness.observation.receipt.v1";
const PROBLEM_SPEC: &str = "canonical.readiness.observation.problem.v1";
const RECORD_DOMAIN: &[u8] = b"canonical.readiness.observation.record.v1\n";
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;
const MAX_ASSERTIONS: usize = 256;
const MAX_EVIDENCE: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_STATEMENT_BYTES: usize = 2_048;
const MAX_LOCATOR_BYTES: usize = 2_048;
const MAX_CONTENT_TYPE_BYTES: usize = 128;
const MEMORY_CAPACITY: usize = 10_000;
const ZERO_RECORD_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct ObservationService {
    database: Option<DatabaseConnection>,
    keys: Arc<KeyRing>,
    memory: Arc<Mutex<MemoryStore>>,
    allow_memory: bool,
}

impl fmt::Debug for ObservationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservationService")
            .field("database_configured", &self.database.is_some())
            .field("configured_sources", &self.keys.by_source.len())
            .field("allow_memory", &self.allow_memory)
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
            allow_memory: false,
        })
    }

    #[cfg(test)]
    fn with_key_document(document: &str) -> Result<Self, BuildError> {
        Ok(Self {
            database: None,
            keys: Arc::new(KeyRing::parse(document)?),
            memory: Arc::new(Mutex::new(MemoryStore::default())),
            allow_memory: true,
        })
    }

    async fn ingest(
        &self,
        path_source_id: &str,
        headers: &HeaderMap,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Result<ObservationReceipt, IngestError> {
        validate_content_type(headers)?;
        if body.is_empty() {
            return Err(IngestError::InvalidRequest);
        }
        if body.len() > MAX_BODY_BYTES {
            return Err(IngestError::BodyTooLarge);
        }

        let event: ObservationEvent =
            serde_json::from_slice(body).map_err(|_| IngestError::InvalidRequest)?;
        validate_event(&event, now)?;
        if path_source_id != event.source_id {
            return Err(IngestError::InvalidRequest);
        }

        let transport = self.keys.verify(headers, body, &event, now)?;
        let payload_sha256 = sha256_label(body);
        let candidate = ReceiptIdentity {
            receipt_id: format!("rcpt_{}", Uuid::new_v4().simple()),
            received_at: format_timestamp(now)?,
        };
        let stored = match &self.database {
            Some(database) => {
                store_database(
                    database,
                    &transport,
                    &event,
                    &payload_sha256,
                    body.len(),
                    &candidate,
                )
                .await?
            }
            None if self.allow_memory => {
                self.store_memory(&transport, &event, &payload_sha256, candidate)?
            }
            None => return Err(IngestError::StorageUnavailable),
        };

        Ok(ObservationReceipt {
            spec_version: RECEIPT_SPEC,
            receipt_id: stored.receipt_id,
            event_id: event.event_id,
            source_id: event.source_id,
            source_sequence: event.source_sequence,
            status: if stored.duplicate {
                "duplicate"
            } else {
                "accepted"
            },
            duplicate: stored.duplicate,
            payload_sha256,
            received_at: stored.received_at,
            transport_verification: "signature-valid",
            substantive_review: "unreviewed",
        })
    }

    fn store_memory(
        &self,
        transport: &VerifiedTransport,
        event: &ObservationEvent,
        payload_sha256: &str,
        candidate: ReceiptIdentity,
    ) -> Result<StorageOutcome, IngestError> {
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
            return if existing.payload_sha256 == payload_sha256
                && existing.source_sequence == event.source_sequence
            {
                Ok(StorageOutcome {
                    duplicate: true,
                    receipt_id: existing.receipt_id.clone(),
                    received_at: existing.received_at.clone(),
                })
            } else {
                Err(IngestError::EventConflict)
            };
        }

        let stream_key = (transport.owner_subject.clone(), event.source_id.clone());
        let (prior_sequence, prior_record_sha256) = memory
            .streams
            .get(&stream_key)
            .map(|cursor| (cursor.source_sequence, cursor.record_sha256.as_str()))
            .unwrap_or((0, ZERO_RECORD_DIGEST));
        let expected = prior_sequence
            .checked_add(1)
            .ok_or(IngestError::SequenceConflict)?;
        if event.source_sequence != expected {
            return Err(IngestError::SequenceConflict);
        }
        if memory.order.len() >= MEMORY_CAPACITY {
            return Err(IngestError::StorageUnavailable);
        }

        let record_sha256 = record_digest(
            prior_record_sha256,
            payload_sha256,
            &event.event_id,
            event.source_sequence,
        );
        memory.events.insert(
            event_key.clone(),
            MemoryRecord {
                source_sequence: event.source_sequence,
                payload_sha256: payload_sha256.into(),
                receipt_id: candidate.receipt_id.clone(),
                received_at: candidate.received_at.clone(),
            },
        );
        memory.streams.insert(
            stream_key,
            StreamCursor {
                source_sequence: event.source_sequence,
                record_sha256,
            },
        );
        memory.order.push_back(event_key);
        Ok(StorageOutcome {
            duplicate: false,
            receipt_id: candidate.receipt_id,
            received_at: candidate.received_at,
        })
    }
}

pub fn router(service: ObservationService) -> Router {
    Router::new()
        .route(
            "/api/v1/readiness/sources/{source_id}/observations",
            post(ingest_observation),
        )
        .route(
            "/v1/readiness/sources/{source_id}/observations",
            post(ingest_observation),
        )
        .with_state(service)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::map_response(observation_security_headers))
}

async fn ingest_observation(
    State(service): State<ObservationService>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<ObservationReceipt>), IngestProblem> {
    let receipt = service
        .ingest(&source_id, &headers, &body, OffsetDateTime::now_utc())
        .await
        .map_err(IngestProblem::from)?;
    Ok((StatusCode::ACCEPTED, Json(receipt)))
}

async fn observation_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; base-uri 'none'; frame-ancestors 'none'"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}

#[derive(Default)]
struct MemoryStore {
    events: HashMap<(String, String, String), MemoryRecord>,
    streams: HashMap<(String, String), StreamCursor>,
    order: VecDeque<(String, String, String)>,
}

struct MemoryRecord {
    source_sequence: i64,
    payload_sha256: String,
    receipt_id: String,
    received_at: String,
}

struct StreamCursor {
    source_sequence: i64,
    record_sha256: String,
}

struct ReceiptIdentity {
    receipt_id: String,
    received_at: String,
}

struct StorageOutcome {
    duplicate: bool,
    receipt_id: String,
    received_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    by_source: HashMap<String, Vec<IngestKey>>,
}

struct IngestKey {
    key_id: String,
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

        let mut key_ids = HashSet::new();
        let mut by_source: HashMap<String, Vec<IngestKey>> = HashMap::new();
        for input in document.keys {
            if !portable_identifier(&input.key_id)
                || !portable_identifier(&input.source_id)
                || !portable_identifier(&input.organization)
                || !valid_subject(&input.owner_subject)
                || !key_ids.insert(input.key_id.clone())
            {
                return Err(BuildError::InvalidKeyDocument);
            }
            let secret = input.secret.into_bytes();
            if secret.len() < 32 || secret.iter().any(|byte| !byte.is_ascii_graphic()) {
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
            by_source
                .entry(input.source_id)
                .or_default()
                .push(IngestKey {
                    key_id: input.key_id,
                    organization: input.organization,
                    owner_subject: input.owner_subject,
                    secret,
                    valid_from,
                    valid_until,
                });
        }
        Ok(Self { by_source })
    }

    fn verify(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        event: &ObservationEvent,
        now: OffsetDateTime,
    ) -> Result<VerifiedTransport, IngestError> {
        let webhook_id = header(headers, "x-canonical-webhook-id")?;
        if webhook_id != event.event_id {
            return Err(IngestError::InvalidSignature);
        }
        let timestamp_text = header(headers, "x-canonical-webhook-timestamp")?;
        let timestamp = timestamp_text
            .parse::<i64>()
            .map_err(|_| IngestError::InvalidSignature)?;
        if now.unix_timestamp().abs_diff(timestamp) > MAX_CLOCK_SKEW_SECONDS as u64 {
            return Err(IngestError::StaleSignature);
        }
        let supplied = parse_signature(header(headers, "x-canonical-webhook-signature")?)?;
        let candidates = self
            .by_source
            .get(&event.source_id)
            .ok_or(IngestError::InvalidSignature)?;

        for key in candidates {
            if key.organization != event.organization
                || key.valid_from.is_some_and(|start| now < start)
                || key.valid_until.is_some_and(|end| now >= end)
            {
                continue;
            }
            let mut mac =
                HmacSha256::new_from_slice(&key.secret).map_err(|_| IngestError::Internal)?;
            mac.update(timestamp_text.as_bytes());
            mac.update(b".");
            mac.update(body);
            if mac.verify_slice(&supplied).is_ok() {
                return Ok(VerifiedTransport {
                    key_id: key.key_id.clone(),
                    owner_subject: key.owner_subject.clone(),
                });
            }
        }
        Err(IngestError::InvalidSignature)
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
        || !portable_identifier(&event.event_id)
        || !portable_identifier(&event.source_id)
        || !portable_identifier(&event.organization)
        || event.source_sequence <= 0
        || event.assertions.is_empty()
        || event.assertions.len() > MAX_ASSERTIONS
        || event.evidence.len() > MAX_EVIDENCE
    {
        return Err(IngestError::InvalidRequest);
    }

    let observed_at =
        parse_timestamp(&event.observed_at).map_err(|_| IngestError::InvalidRequest)?;
    if observed_at.unix_timestamp() - now.unix_timestamp() > MAX_CLOCK_SKEW_SECONDS {
        return Err(IngestError::InvalidRequest);
    }

    let mut evidence_ids = HashSet::new();
    for evidence in &event.evidence {
        if !portable_identifier(&evidence.evidence_id)
            || evidence.kind.is_empty()
            || evidence.kind.len() > 64
            || !valid_sha256(&evidence.sha256)
            || evidence.content_type.as_ref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > MAX_CONTENT_TYPE_BYTES
                    || value.bytes().any(|byte| byte.is_ascii_control())
            })
            || evidence.size_bytes.is_some_and(|size| size > (1_u64 << 50))
            || !evidence_ids.insert(evidence.evidence_id.as_str())
        {
            return Err(IngestError::InvalidRequest);
        }
        let collected_at =
            parse_timestamp(&evidence.collected_at).map_err(|_| IngestError::InvalidRequest)?;
        if collected_at.unix_timestamp() - now.unix_timestamp() > MAX_CLOCK_SKEW_SECONDS
            || collected_at.unix_timestamp() - observed_at.unix_timestamp() > MAX_CLOCK_SKEW_SECONDS
        {
            return Err(IngestError::InvalidRequest);
        }
        if let Some(locator) = &evidence.locator {
            validate_locator(locator)?;
        }
    }

    let mut assertions = HashSet::new();
    for assertion in &event.assertions {
        if !portable_identifier(&assertion.framework_id)
            || !portable_identifier(&assertion.control_id)
            || assertion
                .statement
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > MAX_STATEMENT_BYTES)
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
    if locator.is_empty() || locator.len() > MAX_LOCATOR_BYTES {
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
    candidate: &ReceiptIdentity,
) -> Result<StorageOutcome, IngestError> {
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
            SELECT
                payload_sha256,
                source_sequence,
                receipt_id,
                to_char(
                    received_at AT TIME ZONE 'UTC',
                    'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'
                ) AS received_at_text
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
        let stored_payload: String = row.try_get("", "payload_sha256")?;
        let stored_sequence: i64 = row.try_get("", "source_sequence")?;
        if stored_payload == payload_sha256 && stored_sequence == event.source_sequence {
            let outcome = StorageOutcome {
                duplicate: true,
                receipt_id: row.try_get("", "receipt_id")?,
                received_at: row.try_get("", "received_at_text")?,
            };
            transaction.commit().await?;
            return Ok(outcome);
        }
        transaction.rollback().await?;
        return Err(IngestError::EventConflict);
    }

    let cursor = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT source_sequence, record_sha256
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
    let (prior_sequence, prior_record_sha256) = match cursor {
        Some(row) => (
            row.try_get::<i64>("", "source_sequence")?,
            row.try_get::<String>("", "record_sha256")?,
        ),
        None => (0, ZERO_RECORD_DIGEST.to_owned()),
    };
    let expected = prior_sequence
        .checked_add(1)
        .ok_or(IngestError::SequenceConflict)?;
    if event.source_sequence != expected {
        transaction.rollback().await?;
        return Err(IngestError::SequenceConflict);
    }

    let event_json: JsonValue = serde_json::to_value(event).map_err(|_| IngestError::Internal)?;
    let record_sha256 = record_digest(
        &prior_record_sha256,
        payload_sha256,
        &event.event_id,
        event.source_sequence,
    );
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
                prior_record_sha256,
                record_sha256,
                receipt_id,
                key_id,
                event_json,
                raw_body_octets,
                transport_verification,
                substantive_review,
                received_at
            )
            VALUES (
                $1, $2, $3, $4, $5, CAST($6 AS timestamptz), $7, $8, $9,
                $10, $11, $12, $13, 'signature-valid', 'unreviewed',
                CAST($14 AS timestamptz)
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
                prior_record_sha256.into(),
                record_sha256.into(),
                candidate.receipt_id.clone().into(),
                transport.key_id.clone().into(),
                event_json.into(),
                i64::try_from(body_len)
                    .map_err(|_| IngestError::BodyTooLarge)?
                    .into(),
                candidate.received_at.clone().into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(StorageOutcome {
        duplicate: false,
        receipt_id: candidate.receipt_id.clone(),
        received_at: candidate.received_at.clone(),
    })
}

async fn set_subject(transaction: &DatabaseTransaction, subject: &str) -> Result<(), IngestError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.current_subject', $1, true)",
            [subject.to_owned().into()],
        ))
        .await?;
    Ok(())
}

fn validate_content_type(headers: &HeaderMap) -> Result<(), IngestError> {
    let value = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .ok_or(IngestError::InvalidRequest)?;
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    if media_type.eq_ignore_ascii_case("application/json") {
        Ok(())
    } else {
        Err(IngestError::InvalidRequest)
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, IngestError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or(IngestError::InvalidSignature)
}

fn parse_signature(value: &str) -> Result<[u8; 32], IngestError> {
    let hex = value
        .strip_prefix("v1=")
        .filter(|value| value.len() == 64)
        .ok_or(IngestError::InvalidSignature)?;
    let bytes = hex.as_bytes();
    let mut output = [0_u8; 32];
    for (index, destination) in output.iter_mut().enumerate() {
        let high = decode_nibble(bytes[index * 2]).ok_or(IngestError::InvalidSignature)?;
        let low = decode_nibble(bytes[index * 2 + 1]).ok_or(IngestError::InvalidSignature)?;
        *destination = (high << 4) | low;
    }
    Ok(output)
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn portable_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}

fn valid_subject(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(is_lower_hex))
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(value, &Rfc3339)
}

fn format_timestamp(value: OffsetDateTime) -> Result<String, IngestError> {
    value.format(&Rfc3339).map_err(|_| IngestError::Internal)
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_lower(&Sha256::digest(bytes)))
}

fn record_digest(
    prior_record_sha256: &str,
    payload_sha256: &str,
    event_id: &str,
    source_sequence: i64,
) -> String {
    let sequence = source_sequence.to_string();
    let mut digest = Sha256::new();
    digest.update(RECORD_DOMAIN);
    for value in [prior_record_sha256, payload_sha256, event_id, &sequence] {
        digest.update(value.as_bytes());
        digest.update(b"\n");
    }
    format!("sha256:{}", hex_lower(&digest.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("readiness key document is invalid")]
    InvalidKeyDocument,
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
    #[error("readiness observation body is too large")]
    BodyTooLarge,
    #[error("webhook authentication failed")]
    InvalidSignature,
    #[error("webhook timestamp is outside the replay window")]
    StaleSignature,
    #[error("event id was reused with different content")]
    EventConflict,
    #[error("source sequence must be the next contiguous value")]
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
            IngestError::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid-request",
                "readiness observation body exceeds the configured limit",
            ),
            IngestError::InvalidSignature => (
                StatusCode::UNAUTHORIZED,
                "invalid-signature",
                "webhook authentication failed",
            ),
            IngestError::StaleSignature => (
                StatusCode::UNAUTHORIZED,
                "stale-signature",
                "webhook timestamp is outside the replay window",
            ),
            IngestError::EventConflict => (
                StatusCode::CONFLICT,
                "event-conflict",
                "event id was reused with different content",
            ),
            IngestError::SequenceConflict => (
                StatusCode::CONFLICT,
                "sequence-conflict",
                "source sequence must be the next contiguous value",
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

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn event(sequence: i64) -> ObservationEvent {
        ObservationEvent {
            spec_version: EVENT_SPEC.into(),
            event_id: format!("event.test.{sequence:08}"),
            source_id: "source.customer-ci".into(),
            source_sequence: sequence,
            organization: "org:example".into(),
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
            r#"{{"keys":[{{"keyId":"key.customer-ci","sourceId":"source.customer-ci","organization":"org:example","ownerSubject":"org:example","secret":"{SECRET}"}}]}}"#
        );
        ObservationService::with_key_document(&document).unwrap()
    }

    fn signed_headers(body: &[u8], event_id: &str, timestamp: i64) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("x-canonical-webhook-id", event_id.parse().unwrap());
        headers.insert(
            "x-canonical-webhook-timestamp",
            timestamp.to_string().parse().unwrap(),
        );
        let timestamp_text = timestamp.to_string();
        let mut mac = HmacSha256::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(timestamp_text.as_bytes());
        mac.update(b".");
        mac.update(body);
        headers.insert(
            "x-canonical-webhook-signature",
            format!("v1={}", hex_lower(&mac.finalize().into_bytes()))
                .parse()
                .unwrap(),
        );
        headers
    }

    #[test]
    fn signature_parser_rejects_noncanonical_hex() {
        assert!(parse_signature(&format!("v1={}", "a".repeat(64))).is_ok());
        assert!(parse_signature(&format!("v1={}", "A".repeat(64))).is_err());
        assert!(parse_signature("v1=short").is_err());
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

    #[test]
    fn record_digest_chains_prior_and_payload_state() {
        let first = record_digest(
            ZERO_RECORD_DIGEST,
            &format!("sha256:{}", "a".repeat(64)),
            "event.test.00000001",
            1,
        );
        let changed_prior = record_digest(
            &format!("sha256:{}", "b".repeat(64)),
            &format!("sha256:{}", "a".repeat(64)),
            "event.test.00000001",
            1,
        );
        assert!(valid_sha256(&first));
        assert_ne!(first, changed_prior);
    }

    #[tokio::test]
    async fn exact_replay_returns_the_original_receipt() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let service = service();
        let candidate = event(1);
        let body = serde_json::to_vec(&candidate).unwrap();
        let headers = signed_headers(&body, &candidate.event_id, now.unix_timestamp());
        let first = service
            .ingest(&candidate.source_id, &headers, &body, now)
            .await
            .unwrap();
        let second = service
            .ingest(&candidate.source_id, &headers, &body, now)
            .await
            .unwrap();
        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(first.receipt_id, second.receipt_id);
        assert_eq!(first.received_at, second.received_at);
    }

    #[tokio::test]
    async fn event_mutation_conflicts() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let service = service();
        let candidate = event(1);
        let body = serde_json::to_vec(&candidate).unwrap();
        let headers = signed_headers(&body, &candidate.event_id, now.unix_timestamp());
        service
            .ingest(&candidate.source_id, &headers, &body, now)
            .await
            .unwrap();

        let mut altered = candidate;
        altered.assertions[0].reported_status = ReportedStatus::Unknown;
        let altered_body = serde_json::to_vec(&altered).unwrap();
        let altered_headers =
            signed_headers(&altered_body, &altered.event_id, now.unix_timestamp());
        assert!(matches!(
            service
                .ingest(&altered.source_id, &altered_headers, &altered_body, now)
                .await,
            Err(IngestError::EventConflict)
        ));
    }

    #[tokio::test]
    async fn sequence_must_begin_at_one_and_remain_contiguous() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let service = service();
        let skipped_first = event(2);
        let body = serde_json::to_vec(&skipped_first).unwrap();
        let headers = signed_headers(&body, &skipped_first.event_id, now.unix_timestamp());
        assert!(matches!(
            service
                .ingest(&skipped_first.source_id, &headers, &body, now)
                .await,
            Err(IngestError::SequenceConflict)
        ));

        let first = event(1);
        let body = serde_json::to_vec(&first).unwrap();
        let headers = signed_headers(&body, &first.event_id, now.unix_timestamp());
        service
            .ingest(&first.source_id, &headers, &body, now)
            .await
            .unwrap();

        let gap = event(3);
        let body = serde_json::to_vec(&gap).unwrap();
        let headers = signed_headers(&body, &gap.event_id, now.unix_timestamp());
        assert!(matches!(
            service.ingest(&gap.source_id, &headers, &body, now).await,
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
            service()
                .ingest(&candidate.source_id, &headers, &body, now)
                .await,
            Err(IngestError::StaleSignature)
        ));
    }

    #[tokio::test]
    async fn production_service_fails_closed_without_durable_storage() {
        let now = parse_timestamp("2026-09-09T05:00:00Z").unwrap();
        let document = format!(
            r#"{{"keys":[{{"keyId":"key.customer-ci","sourceId":"source.customer-ci","organization":"org:example","ownerSubject":"org:example","secret":"{SECRET}"}}]}}"#
        );
        let mut service = ObservationService::with_key_document(&document).unwrap();
        service.allow_memory = false;
        let candidate = event(1);
        let body = serde_json::to_vec(&candidate).unwrap();
        let headers = signed_headers(&body, &candidate.event_id, now.unix_timestamp());
        assert!(matches!(
            service
                .ingest(&candidate.source_id, &headers, &body, now)
                .await,
            Err(IngestError::StorageUnavailable)
        ));
    }
}
