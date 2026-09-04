use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult,
    Statement, TransactionTrait,
};
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CanonicalContext, CreateQuoteRequest, QuoteRecord, QuoteRequest};

const MAX_JOB_ATTEMPTS: i32 = 5;
// Gemini permits three bounded 70-second attempts plus backoff. The lease must
// exceed that complete provider budget so another worker cannot duplicate an
// in-flight analysis before the first worker can persist its terminal state.
const LEASE_SECONDS: i32 = 300;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("verified contact selection was not found or expired")]
    ContactSelectionUnavailable,
    #[error("canonical context was not found")]
    ContextNotFound,
    #[error("database operation failed")]
    Database(#[from] DbErr),
    #[error("stored record is invalid: {0}")]
    InvalidRecord(&'static str),
    #[error("quote access link was not found or expired")]
    LinkNotFound,
    #[error("quote was not found")]
    QuoteNotFound,
    #[error("quote is not in an editable terminal state")]
    QuoteNotEditable,
}

impl StoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ContactSelectionUnavailable => "contact_selection_unavailable",
            Self::ContextNotFound => "context_not_found",
            Self::Database(_) => "database_error",
            Self::InvalidRecord(_) => "invalid_stored_record",
            Self::LinkNotFound => "link_not_found",
            Self::QuoteNotFound => "quote_not_found",
            Self::QuoteNotEditable => "quote_not_editable",
        }
    }
}

pub struct NewContactSelection<'a> {
    pub email_ciphertext: &'a str,
    pub email_masked: &'a str,
    pub expires_at: OffsetDateTime,
    pub id: Uuid,
    pub phone_ciphertext: &'a str,
    pub phone_masked: &'a str,
}

pub async fn create_contact_selection(
    database: &DatabaseConnection,
    subject: &str,
    selection: NewContactSelection<'_>,
) -> Result<(), StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_contact_selection (
                id, owner_subject, email_ciphertext, phone_ciphertext,
                email_masked, phone_masked, email_confirmed_at,
                phone_confirmed_at, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, $7)
            "#,
            [
                selection.id.into(),
                subject.to_owned().into(),
                selection.email_ciphertext.to_owned().into(),
                selection.phone_ciphertext.to_owned().into(),
                selection.email_masked.to_owned().into(),
                selection.phone_masked.to_owned().into(),
                selection.expires_at.into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub struct NewQuoteAccess<'a> {
    pub expires_at: OffsetDateTime,
    pub id: Uuid,
    pub token_ciphertext: &'a str,
    pub token_digest: &'a str,
}

pub async fn create_quote(
    database: &DatabaseConnection,
    subject: &str,
    request: &QuoteRequest,
    analysis_request: &CreateQuoteRequest,
    record: &QuoteRecord,
    access: NewQuoteAccess<'_>,
) -> Result<CanonicalContext, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;

    let context_row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, name, context_markdown, context_json
            FROM canonical_context
            WHERE owner_subject = $1 AND active = TRUE
            ORDER BY updated_at DESC, id
            LIMIT 1
            "#,
            [subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::ContextNotFound)?;
    let context = context_from_row(&context_row)?;
    context.validate().map_err(StoreError::InvalidRecord)?;

    let contact_exists = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id
            FROM canonical_quote_contact_selection
            WHERE id = $1
              AND owner_subject = $2
              AND expires_at > CURRENT_TIMESTAMP
              AND consumed_quote_id IS NULL
            FOR UPDATE
            "#,
            [
                request.contact_selection_id.into(),
                subject.to_owned().into(),
            ],
        ))
        .await?;
    if contact_exists.is_none() {
        transaction.rollback().await?;
        return Err(StoreError::ContactSelectionUnavailable);
    }

    let request_json =
        serde_json::to_value(request).map_err(|_| StoreError::InvalidRecord("quote request"))?;
    let insert_values = [
        record.quote_id.into(),
        subject.to_owned().into(),
        request.contact_selection_id.into(),
        record.revision.into(),
        context.id.into(),
        request_json.clone().into(),
        analysis_request.markdown_context.clone().into(),
        context.context_markdown.clone().into(),
        context.context_json.clone().into(),
        record.gemini_model.clone().into(),
        record.status.clone().into(),
        access.expires_at.into(),
    ];
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote (
                id, owner_subject, contact_selection_id, current_revision,
                context_record_id, request_json, application_context_markdown,
                context_snapshot_markdown, context_snapshot_json, gemini_model,
                status, analysis_json, error_code, access_link_expires_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NULL,NULL,$12)
            "#,
            insert_values,
        ))
        .await?;

    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_revision (
                quote_id, revision, owner_subject, contact_selection_id,
                context_record_id, request_json, application_context_markdown,
                context_snapshot_markdown, context_snapshot_json, gemini_model,
                status, analysis_json, error_code
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NULL,NULL)
            "#,
            [
                record.quote_id.into(),
                record.revision.into(),
                subject.to_owned().into(),
                request.contact_selection_id.into(),
                context.id.into(),
                request_json.into(),
                analysis_request.markdown_context.clone().into(),
                context.context_markdown.clone().into(),
                context.context_json.clone().into(),
                record.gemini_model.clone().into(),
                record.status.clone().into(),
            ],
        ))
        .await?;

    let consumed = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote_contact_selection
            SET consumed_quote_id = $3, consumed_revision = $4
            WHERE id = $1 AND owner_subject = $2 AND consumed_quote_id IS NULL
            "#,
            [
                request.contact_selection_id.into(),
                subject.to_owned().into(),
                record.quote_id.into(),
                record.revision.into(),
            ],
        ))
        .await?;
    if consumed.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(StoreError::ContactSelectionUnavailable);
    }

    insert_event(&transaction, record).await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_job (id, quote_id, revision, owner_subject)
            VALUES ($1, $2, $3, $4)
            "#,
            [
                Uuid::new_v4().into(),
                record.quote_id.into(),
                record.revision.into(),
                subject.to_owned().into(),
            ],
        ))
        .await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_access_link (
                id, quote_id, owner_subject, token_digest, token_ciphertext, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            [
                access.id.into(),
                record.quote_id.into(),
                subject.to_owned().into(),
                access.token_digest.to_owned().into(),
                access.token_ciphertext.to_owned().into(),
                access.expires_at.into(),
            ],
        ))
        .await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_notification_outbox (
                id, quote_id, revision, owner_subject, access_link_id,
                contact_selection_id, kind, deduplication_key
            )
            VALUES ($1,$2,$3,$4,$5,$6,'sms_quote_link',$7)
            "#,
            [
                Uuid::new_v4().into(),
                record.quote_id.into(),
                record.revision.into(),
                subject.to_owned().into(),
                access.id.into(),
                request.contact_selection_id.into(),
                format!(
                    "quote:{}:{}:sms_quote_link",
                    record.quote_id, record.revision
                )
                .into(),
            ],
        ))
        .await?;

    transaction.commit().await?;
    Ok(context)
}

pub async fn resubmit_quote(
    database: &DatabaseConnection,
    subject: &str,
    request: &QuoteRequest,
    analysis_request: &CreateQuoteRequest,
    record: &QuoteRecord,
) -> Result<CanonicalContext, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let quote_row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT current_revision, status
            FROM canonical_quote
            WHERE id = $1 AND owner_subject = $2
            FOR UPDATE
            "#,
            [record.quote_id.into(), subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    let current_revision = quote_row.try_get::<i32>("", "current_revision")?;
    let current_status = quote_row.try_get::<String>("", "status")?;
    if record.revision != current_revision + 1
        || !matches!(current_status.as_str(), "ready" | "failed")
    {
        transaction.rollback().await?;
        return Err(StoreError::QuoteNotEditable);
    }

    let contact_row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT consumed_quote_id
            FROM canonical_quote_contact_selection
            WHERE id = $1
              AND owner_subject = $2
              AND (
                consumed_quote_id = $3
                OR (consumed_quote_id IS NULL AND expires_at > CURRENT_TIMESTAMP)
              )
            FOR UPDATE
            "#,
            [
                request.contact_selection_id.into(),
                subject.to_owned().into(),
                record.quote_id.into(),
            ],
        ))
        .await?
        .ok_or(StoreError::ContactSelectionUnavailable)?;
    let consumed_quote_id = contact_row.try_get::<Option<Uuid>>("", "consumed_quote_id")?;

    let context_row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, name, context_markdown, context_json
            FROM canonical_context
            WHERE owner_subject = $1 AND active = TRUE
            ORDER BY updated_at DESC, id
            LIMIT 1
            "#,
            [subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::ContextNotFound)?;
    let context = context_from_row(&context_row)?;
    context.validate().map_err(StoreError::InvalidRecord)?;
    let request_json =
        serde_json::to_value(request).map_err(|_| StoreError::InvalidRecord("quote request"))?;

    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_revision (
                quote_id, revision, owner_subject, contact_selection_id,
                context_record_id, request_json, application_context_markdown,
                context_snapshot_markdown, context_snapshot_json, gemini_model,
                status, analysis_json, error_code
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'queued',NULL,NULL)
            "#,
            [
                record.quote_id.into(),
                record.revision.into(),
                subject.to_owned().into(),
                request.contact_selection_id.into(),
                context.id.into(),
                request_json.clone().into(),
                analysis_request.markdown_context.clone().into(),
                context.context_markdown.clone().into(),
                context.context_json.clone().into(),
                record.gemini_model.clone().into(),
            ],
        ))
        .await?;
    let updated = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote
            SET contact_selection_id = $4,
                current_revision = $3,
                context_record_id = $5,
                request_json = $6,
                application_context_markdown = $7,
                context_snapshot_markdown = $8,
                context_snapshot_json = $9,
                gemini_model = $10,
                status = 'queued',
                analysis_json = NULL,
                error_code = NULL
            WHERE id = $1 AND owner_subject = $2 AND current_revision = $3 - 1
            "#,
            [
                record.quote_id.into(),
                subject.to_owned().into(),
                record.revision.into(),
                request.contact_selection_id.into(),
                context.id.into(),
                request_json.into(),
                analysis_request.markdown_context.clone().into(),
                context.context_markdown.clone().into(),
                context.context_json.clone().into(),
                record.gemini_model.clone().into(),
            ],
        ))
        .await?;
    if updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(StoreError::QuoteNotEditable);
    }

    if consumed_quote_id.is_none() {
        let consumed = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                UPDATE canonical_quote_contact_selection
                SET consumed_quote_id = $3, consumed_revision = $4
                WHERE id = $1 AND owner_subject = $2 AND consumed_quote_id IS NULL
                "#,
                [
                    request.contact_selection_id.into(),
                    subject.to_owned().into(),
                    record.quote_id.into(),
                    record.revision.into(),
                ],
            ))
            .await?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StoreError::ContactSelectionUnavailable);
        }
    }

    insert_event(&transaction, record).await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_job (id, quote_id, revision, owner_subject)
            VALUES ($1, $2, $3, $4)
            "#,
            [
                Uuid::new_v4().into(),
                record.quote_id.into(),
                record.revision.into(),
                subject.to_owned().into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(context)
}

pub async fn get_quote(
    database: &DatabaseConnection,
    subject: &str,
    quote_id: Uuid,
) -> Result<QuoteRecord, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            quote_projection_sql("WHERE id = $1 AND owner_subject = $2"),
            [quote_id.into(), subject.to_owned().into()],
        ))
        .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Err(StoreError::QuoteNotFound);
    };
    let record = quote_from_row(&row)?;
    transaction.commit().await?;
    Ok(record)
}

pub async fn list_quotes(
    database: &DatabaseConnection,
    subject: &str,
    limit: u64,
) -> Result<Vec<QuoteRecord>, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let rows = transaction
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            quote_projection_sql("WHERE owner_subject = $1 ORDER BY created_at DESC LIMIT $2"),
            [
                subject.to_owned().into(),
                i64::try_from(limit.clamp(1, 100)).unwrap_or(100).into(),
            ],
        ))
        .await?;
    let records = rows
        .iter()
        .map(quote_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await?;
    Ok(records)
}

pub async fn update_quote(
    database: &DatabaseConnection,
    record: &QuoteRecord,
) -> Result<(), StoreError> {
    let transaction = begin_subject_transaction(database, &record.owner_subject).await?;
    let revision_result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote_revision
            SET status = $4, analysis_json = $5, error_code = $6
            WHERE quote_id = $1 AND revision = $2 AND owner_subject = $3
            "#,
            [
                record.quote_id.into(),
                record.revision.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                record.analysis.clone().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?;
    let quote_result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote
            SET status = $4, analysis_json = $5, error_code = $6
            WHERE id = $1 AND current_revision = $2 AND owner_subject = $3
            "#,
            [
                record.quote_id.into(),
                record.revision.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                record.analysis.clone().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?;
    if revision_result.rows_affected() != 1 || quote_result.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(StoreError::QuoteNotFound);
    }
    insert_event(&transaction, record).await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn start_model_attempt(
    database: &DatabaseConnection,
    record: &QuoteRecord,
) -> Result<Uuid, StoreError> {
    let attempt_id = Uuid::new_v4();
    let transaction = begin_subject_transaction(database, &record.owner_subject).await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_model_attempt (
                id, quote_id, revision, owner_subject, provider, model, status
            ) VALUES ($1,$2,$3,$4,'google-gemini',$5,'started')
            "#,
            [
                attempt_id.into(),
                record.quote_id.into(),
                record.revision.into(),
                record.owner_subject.clone().into(),
                record.gemini_model.clone().into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(attempt_id)
}

pub async fn finish_model_attempt(
    database: &DatabaseConnection,
    record: &QuoteRecord,
    attempt_id: Uuid,
) -> Result<(), StoreError> {
    let attempt_status = if record.status == "ready" {
        "completed"
    } else {
        "failed"
    };
    let transaction = begin_subject_transaction(database, &record.owner_subject).await?;
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_model_attempt
            SET status = $5, error_code = $6, finished_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND quote_id = $2 AND revision = $3
              AND owner_subject = $4 AND status = 'started'
            "#,
            [
                attempt_id.into(),
                record.quote_id.into(),
                record.revision.into(),
                record.owner_subject.clone().into(),
                attempt_status.to_owned().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(StoreError::InvalidRecord("model attempt transition"));
    }
    transaction.commit().await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ClaimedQuoteJob {
    pub id: Uuid,
    pub owner_subject: String,
    pub quote_id: Uuid,
    pub revision: i32,
    pub worker_id: Uuid,
}

pub async fn claim_quote_job(
    database: &DatabaseConnection,
    worker_id: Uuid,
) -> Result<Option<ClaimedQuoteJob>, StoreError> {
    ensure_postgres(database)?;
    let transaction = database.begin().await?;
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            WITH candidate AS (
                SELECT id
                FROM canonical_quote_job
                WHERE attempt_count < $2
                  AND next_attempt_at <= CURRENT_TIMESTAMP
                  AND (
                    status = 'queued'
                    OR (status = 'running' AND lease_expires_at < CURRENT_TIMESTAMP)
                  )
                ORDER BY next_attempt_at, created_at, id
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            UPDATE canonical_quote_job AS job
            SET status = 'running',
                attempt_count = attempt_count + 1,
                lease_owner = $1,
                lease_expires_at = CURRENT_TIMESTAMP + ($3 * INTERVAL '1 second'),
                last_error_code = NULL
            FROM candidate
            WHERE job.id = candidate.id
            RETURNING job.id, job.quote_id, job.revision, job.owner_subject
            "#,
            [
                worker_id.into(),
                MAX_JOB_ATTEMPTS.into(),
                LEASE_SECONDS.into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    row.map(|row| {
        Ok(ClaimedQuoteJob {
            id: row.try_get("", "id")?,
            owner_subject: row.try_get("", "owner_subject")?,
            quote_id: row.try_get("", "quote_id")?,
            revision: row.try_get("", "revision")?,
            worker_id,
        })
    })
    .transpose()
}

pub async fn load_claimed_quote(
    database: &DatabaseConnection,
    job: &ClaimedQuoteJob,
) -> Result<(QuoteRecord, CreateQuoteRequest, CanonicalContext), StoreError> {
    ensure_postgres(database)?;
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
                revision.quote_id AS id,
                revision.revision,
                revision.owner_subject,
                revision.context_record_id,
                revision.request_json,
                revision.application_context_markdown,
                revision.context_snapshot_markdown,
                revision.context_snapshot_json,
                revision.gemini_model,
                revision.status,
                revision.analysis_json,
                revision.error_code,
                quote.access_link_expires_at,
                quote.created_at,
                revision.updated_at
            FROM canonical_quote_revision AS revision
            JOIN canonical_quote AS quote
              ON quote.id = revision.quote_id AND quote.owner_subject = revision.owner_subject
            JOIN canonical_quote_job AS job
              ON job.quote_id = revision.quote_id AND job.revision = revision.revision
            WHERE job.id = $1 AND job.lease_owner = $2 AND job.status = 'running'
            "#,
            [job.id.into(), job.worker_id.into()],
        ))
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    let record = quote_from_row(&row)?;
    if record.owner_subject != job.owner_subject {
        return Err(StoreError::InvalidRecord("quote job owner"));
    }
    let analysis_request = record
        .request
        .to_analysis_request()
        .map_err(StoreError::InvalidRecord)?;
    let context = CanonicalContext {
        context_json: row.try_get("", "context_snapshot_json")?,
        context_markdown: row.try_get("", "context_snapshot_markdown")?,
        id: row.try_get("", "context_record_id")?,
        name: "immutable quote context snapshot".into(),
    };
    Ok((record, analysis_request, context))
}

pub async fn finish_quote_job(
    database: &DatabaseConnection,
    job: &ClaimedQuoteJob,
    record: &QuoteRecord,
) -> Result<(), StoreError> {
    ensure_postgres(database)?;
    let (status, error_code) = if record.status == "ready" {
        ("completed", None)
    } else {
        ("failed", record.error_code.clone())
    };
    let result = database
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote_job
            SET status = $3, lease_owner = NULL, lease_expires_at = NULL,
                last_error_code = $4
            WHERE id = $1 AND lease_owner = $2 AND status = 'running'
            "#,
            [
                job.id.into(),
                job.worker_id.into(),
                status.to_owned().into(),
                error_code.into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
        return Err(StoreError::InvalidRecord("quote job lease"));
    }
    Ok(())
}

#[derive(Clone)]
pub struct ClaimedNotification {
    pub access_link_id: Uuid,
    pub contact_selection_id: Uuid,
    pub id: Uuid,
    pub owner_subject: String,
    pub phone_ciphertext: String,
    pub quote_id: Uuid,
    pub token_ciphertext: String,
    pub worker_id: Uuid,
}

impl std::fmt::Debug for ClaimedNotification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaimedNotification")
            .field("access_link_id", &self.access_link_id)
            .field("contact_selection_id", &self.contact_selection_id)
            .field("id", &self.id)
            .field("phone_ciphertext", &"[redacted]")
            .field("quote_id", &self.quote_id)
            .field("token_ciphertext", &"[redacted]")
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

pub async fn claim_notification(
    database: &DatabaseConnection,
    worker_id: Uuid,
) -> Result<Option<ClaimedNotification>, StoreError> {
    ensure_postgres(database)?;
    let transaction = database.begin().await?;
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            WITH candidate AS (
                SELECT id
                FROM canonical_notification_outbox
                WHERE attempt_count < $2
                  AND next_attempt_at <= CURRENT_TIMESTAMP
                  AND (
                    status = 'pending'
                    OR (status = 'sending' AND lease_expires_at < CURRENT_TIMESTAMP)
                  )
                ORDER BY next_attempt_at, created_at, id
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            ), claimed AS (
                UPDATE canonical_notification_outbox AS message
                SET status = 'sending', attempt_count = attempt_count + 1,
                    lease_owner = $1,
                    lease_expires_at = CURRENT_TIMESTAMP + ($3 * INTERVAL '1 second'),
                    last_error_code = NULL
                FROM candidate
                WHERE message.id = candidate.id
                RETURNING message.id, message.quote_id, message.owner_subject,
                          message.access_link_id, message.contact_selection_id
            )
            SELECT claimed.id, claimed.quote_id, claimed.owner_subject,
                   claimed.access_link_id, claimed.contact_selection_id,
                   contact.phone_ciphertext, link.token_ciphertext
            FROM claimed
            JOIN canonical_quote_contact_selection AS contact
              ON contact.id = claimed.contact_selection_id
            JOIN canonical_quote_access_link AS link
              ON link.id = claimed.access_link_id
            "#,
            [
                worker_id.into(),
                MAX_JOB_ATTEMPTS.into(),
                LEASE_SECONDS.into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    row.map(|row| {
        Ok(ClaimedNotification {
            access_link_id: row.try_get("", "access_link_id")?,
            contact_selection_id: row.try_get("", "contact_selection_id")?,
            id: row.try_get("", "id")?,
            owner_subject: row.try_get("", "owner_subject")?,
            phone_ciphertext: row.try_get("", "phone_ciphertext")?,
            quote_id: row.try_get("", "quote_id")?,
            token_ciphertext: row.try_get("", "token_ciphertext")?,
            worker_id,
        })
    })
    .transpose()
}

pub async fn finish_notification(
    database: &DatabaseConnection,
    message: &ClaimedNotification,
    provider_message_id: Option<&str>,
    error_code: Option<&str>,
) -> Result<(), StoreError> {
    ensure_postgres(database)?;
    let next_attempt_seconds = if provider_message_id.is_some() {
        0_i32
    } else {
        60_i32
    };
    let result = database
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_notification_outbox
            SET status = CASE
                    WHEN $3 IS NOT NULL THEN 'sent'
                    WHEN attempt_count >= $6 THEN 'failed'
                    ELSE 'pending'
                END,
                lease_owner = NULL,
                lease_expires_at = NULL,
                provider_message_id = $3,
                last_error_code = $4,
                next_attempt_at = CURRENT_TIMESTAMP + ($5 * INTERVAL '1 second')
            WHERE id = $1 AND lease_owner = $2 AND status = 'sending'
            "#,
            [
                message.id.into(),
                message.worker_id.into(),
                provider_message_id.map(str::to_owned).into(),
                error_code.map(str::to_owned).into(),
                next_attempt_seconds.into(),
                MAX_JOB_ATTEMPTS.into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
        return Err(StoreError::InvalidRecord("notification lease"));
    }
    Ok(())
}

#[derive(Debug)]
pub struct RedeemedQuoteLink {
    pub expires_at: OffsetDateTime,
    pub owner_subject: String,
    pub quote_id: Uuid,
}

pub async fn redeem_quote_link(
    database: &DatabaseConnection,
    token_digest: &str,
) -> Result<RedeemedQuoteLink, StoreError> {
    ensure_postgres(database)?;
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT quote_id, owner_subject, expires_at FROM canonical_redeem_quote_link($1)",
            [token_digest.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::LinkNotFound)?;
    Ok(RedeemedQuoteLink {
        expires_at: row.try_get("", "expires_at")?,
        owner_subject: row.try_get("", "owner_subject")?,
        quote_id: row.try_get("", "quote_id")?,
    })
}

async fn begin_subject_transaction(
    database: &DatabaseConnection,
    subject: &str,
) -> Result<DatabaseTransaction, StoreError> {
    ensure_postgres(database)?;
    let transaction = database.begin().await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
              set_config('app.current_subject', $1, true),
              set_config('request.jwt.claim.sub', $1, true),
              set_config('request.jwt.claim.role', 'authenticated', true)
            "#,
            [subject.to_owned().into()],
        ))
        .await?;
    Ok(transaction)
}

fn ensure_postgres(database: &DatabaseConnection) -> Result<(), StoreError> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(StoreError::Database(DbErr::Custom(
            "canonical quote persistence requires PostgreSQL".into(),
        )));
    }
    Ok(())
}

async fn insert_event(
    transaction: &DatabaseTransaction,
    record: &QuoteRecord,
) -> Result<(), StoreError> {
    let details = json!({
        "analysisAvailable": record.analysis.is_some(),
        "errorCode": record.error_code.clone(),
    });
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_event (
                quote_id, revision, owner_subject, status, details_json
            ) VALUES ($1, $2, $3, $4, $5)
            "#,
            [
                record.quote_id.into(),
                record.revision.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                details.into(),
            ],
        ))
        .await?;
    Ok(())
}

fn quote_projection_sql(suffix: &str) -> String {
    format!(
        r#"
        SELECT id, current_revision AS revision, owner_subject,
               context_record_id, request_json, gemini_model, status,
               analysis_json, error_code, access_link_expires_at,
               created_at, updated_at
        FROM canonical_quote
        {suffix}
        "#
    )
}

fn context_from_row(row: &QueryResult) -> Result<CanonicalContext, StoreError> {
    Ok(CanonicalContext {
        context_json: row.try_get::<JsonValue>("", "context_json")?,
        context_markdown: row.try_get::<String>("", "context_markdown")?,
        id: row.try_get::<Uuid>("", "id")?,
        name: row.try_get::<String>("", "name")?,
    })
}

fn quote_from_row(row: &QueryResult) -> Result<QuoteRecord, StoreError> {
    let request_json = row.try_get::<JsonValue>("", "request_json")?;
    let request: QuoteRequest = serde_json::from_value(request_json)
        .map_err(|_| StoreError::InvalidRecord("quote request"))?;
    let status = row.try_get::<String>("", "status")?;
    if !matches!(status.as_str(), "queued" | "analyzing" | "ready" | "failed") {
        return Err(StoreError::InvalidRecord("quote status"));
    }
    Ok(QuoteRecord {
        access_link_expires_at: row.try_get("", "access_link_expires_at")?,
        analysis: row.try_get::<Option<JsonValue>>("", "analysis_json")?,
        context_record_id: row.try_get("", "context_record_id")?,
        created_at: row.try_get("", "created_at")?,
        error_code: row.try_get("", "error_code")?,
        frameworks: request.frameworks.clone(),
        gemini_model: row.try_get("", "gemini_model")?,
        organization_name: request.organization_name.clone(),
        owner_subject: row.try_get("", "owner_subject")?,
        persistence: "postgres".into(),
        quote_id: row.try_get("", "id")?,
        request,
        revision: row.try_get("", "revision")?,
        status,
        updated_at: row.try_get("", "updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::StoreError;

    #[test]
    fn store_errors_expose_bounded_codes() {
        assert_eq!(StoreError::ContextNotFound.code(), "context_not_found");
        assert_eq!(StoreError::QuoteNotFound.code(), "quote_not_found");
        assert_eq!(
            StoreError::ContactSelectionUnavailable.code(),
            "contact_selection_unavailable"
        );
        assert_eq!(StoreError::LinkNotFound.code(), "link_not_found");
        assert_eq!(
            StoreError::InvalidRecord("test").code(),
            "invalid_stored_record"
        );
    }
}
