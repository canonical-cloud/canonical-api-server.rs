use canonical_lib::interfaces::QuoteRequest as WireQuoteRequest;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult,
    Statement, TransactionTrait,
};
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use uuid::Uuid;

use crate::{AcceptedQuoteRequest, CanonicalContext, QuoteEventRecord, QuoteRecord};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("canonical context was not found")]
    ContextNotFound,
    #[error("database operation failed")]
    Database(#[from] DbErr),
    #[error("idempotency key was already used for another operation or payload")]
    IdempotencyKeyReused,
    #[error("stored record is invalid: {0}")]
    InvalidRecord(&'static str),
    #[error("quote transition is not allowed")]
    InvalidTransition,
    #[error("quote was not found")]
    QuoteNotFound,
}

impl StoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ContextNotFound => "context_not_found",
            Self::Database(_) => "database_error",
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::InvalidRecord(_) => "invalid_stored_record",
            Self::InvalidTransition => "invalid_transition",
            Self::QuoteNotFound => "quote_not_found",
        }
    }
}

pub enum CreateQuoteOutcome {
    Created {
        context: CanonicalContext,
        event: QuoteEventRecord,
        record: QuoteRecord,
    },
    Existing {
        record: QuoteRecord,
    },
}

pub enum RetryQuoteOutcome {
    Retried {
        context: CanonicalContext,
        event: QuoteEventRecord,
        record: QuoteRecord,
    },
    Existing {
        record: QuoteRecord,
    },
}

pub async fn create_quote(
    database: &DatabaseConnection,
    subject: &str,
    idempotency_key: &str,
    request: &AcceptedQuoteRequest,
    mut record: QuoteRecord,
) -> Result<CreateQuoteOutcome, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    lock_idempotency(&transaction, subject, idempotency_key).await?;

    let request_json = serde_json::to_value(&request.wire)
        .map_err(|_| StoreError::InvalidRecord("quote request"))?;
    if let Some(existing) = find_operation(&transaction, subject, idempotency_key).await? {
        if existing.operation != "create" || existing.request_json.as_ref() != Some(&request_json) {
            transaction.rollback().await?;
            return Err(StoreError::IdempotencyKeyReused);
        }
        let record = get_quote_in_transaction(&transaction, subject, existing.quote_id).await?;
        transaction.commit().await?;
        return Ok(CreateQuoteOutcome::Existing { record });
    }

    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, name, context_markdown, context_json
            FROM canonical_context
            WHERE owner_subject = $1
              AND active = TRUE
            ORDER BY updated_at DESC, id
            LIMIT 1
            "#,
            [subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::ContextNotFound)?;

    let context = context_from_row(&row)?;
    context.validate().map_err(StoreError::InvalidRecord)?;
    record.context_record_id = context.id;

    let inserted = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote (
                id,
                owner_subject,
                context_record_id,
                request_json,
                application_context_markdown,
                context_snapshot_markdown,
                context_snapshot_json,
                gemini_model,
                status,
                analysis_json,
                error_code
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL)
            RETURNING
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS created_at_text,
                to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS updated_at_text
            "#,
            [
                record.quote_id.into(),
                subject.to_owned().into(),
                context.id.into(),
                request_json.clone().into(),
                request.application_markdown.clone().into(),
                context.context_markdown.clone().into(),
                context.context_json.clone().into(),
                record.gemini_model.clone().into(),
                record.status.clone().into(),
            ],
        ))
        .await?
        .ok_or(StoreError::InvalidRecord("canonical_quote insert"))?;
    record.created_at = inserted.try_get("", "created_at_text")?;
    record.updated_at = inserted.try_get("", "updated_at_text")?;

    insert_operation(
        &transaction,
        subject,
        idempotency_key,
        "create",
        record.quote_id,
        Some(request_json),
    )
    .await?;
    let event = insert_event(&transaction, &record).await?;
    transaction.commit().await?;
    Ok(CreateQuoteOutcome::Created {
        context,
        event,
        record,
    })
}

pub async fn retry_quote(
    database: &DatabaseConnection,
    subject: &str,
    quote_id: Uuid,
    idempotency_key: &str,
) -> Result<RetryQuoteOutcome, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    lock_idempotency(&transaction, subject, idempotency_key).await?;

    if let Some(existing) = find_operation(&transaction, subject, idempotency_key).await? {
        if existing.operation != "retry" || existing.quote_id != quote_id {
            transaction.rollback().await?;
            return Err(StoreError::IdempotencyKeyReused);
        }
        let record = get_quote_in_transaction(&transaction, subject, quote_id).await?;
        transaction.commit().await?;
        return Ok(RetryQuoteOutcome::Existing { record });
    }

    let row = quote_row(&transaction, subject, quote_id, true)
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    let mut record = quote_from_row(&row)?;
    if record.status != "failed" {
        transaction.rollback().await?;
        return Err(StoreError::InvalidTransition);
    }
    let context = context_snapshot_from_row(&row)?;
    context.validate().map_err(StoreError::InvalidRecord)?;

    let updated = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote
            SET status = 'queued',
                analysis_json = NULL,
                error_code = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND owner_subject = $2
            RETURNING
                to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS updated_at_text
            "#,
            [quote_id.into(), subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    record.status = "queued".into();
    record.analysis = None;
    record.error_code = None;
    record.updated_at = updated.try_get("", "updated_at_text")?;

    insert_operation(
        &transaction,
        subject,
        idempotency_key,
        "retry",
        quote_id,
        None,
    )
    .await?;
    let event = insert_event(&transaction, &record).await?;
    transaction.commit().await?;
    Ok(RetryQuoteOutcome::Retried {
        context,
        event,
        record,
    })
}

pub async fn get_quote(
    database: &DatabaseConnection,
    subject: &str,
    quote_id: Uuid,
) -> Result<QuoteRecord, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let record = get_quote_in_transaction(&transaction, subject, quote_id).await?;
    transaction.commit().await?;
    Ok(record)
}

pub async fn list_quotes(
    database: &DatabaseConnection,
    subject: &str,
    limit: u64,
    cursor: Option<Uuid>,
) -> Result<Vec<QuoteRecord>, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let bounded = i64::try_from(limit.clamp(1, 101)).unwrap_or(101);
    let rows = if let Some(cursor) = cursor {
        transaction
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                WITH owner_cursor AS (
                    SELECT created_at, id
                    FROM canonical_quote
                    WHERE owner_subject = $1
                      AND id = $2
                )
                SELECT
                    id,
                    owner_subject,
                    context_record_id,
                    request_json,
                    context_snapshot_markdown,
                    context_snapshot_json,
                    gemini_model,
                    status,
                    analysis_json,
                    error_code,
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS created_at_text,
                    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS updated_at_text
                FROM canonical_quote
                WHERE owner_subject = $1
                  AND (created_at, id) < (
                      SELECT created_at, id FROM owner_cursor
                  )
                ORDER BY created_at DESC, id DESC
                LIMIT $3
                "#,
                [subject.to_owned().into(), cursor.into(), bounded.into()],
            ))
            .await?
    } else {
        transaction
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT
                    id,
                    owner_subject,
                    context_record_id,
                    request_json,
                    context_snapshot_markdown,
                    context_snapshot_json,
                    gemini_model,
                    status,
                    analysis_json,
                    error_code,
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS created_at_text,
                    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS updated_at_text
                FROM canonical_quote
                WHERE owner_subject = $1
                ORDER BY created_at DESC, id DESC
                LIMIT $2
                "#,
                [subject.to_owned().into(), bounded.into()],
            ))
            .await?
    };
    let records = rows
        .iter()
        .map(quote_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await?;
    Ok(records)
}

pub async fn update_quote(
    database: &DatabaseConnection,
    record: &mut QuoteRecord,
) -> Result<QuoteEventRecord, StoreError> {
    let transaction = begin_subject_transaction(database, &record.owner_subject).await?;
    let updated = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote
            SET status = $3,
                analysis_json = $4,
                error_code = $5,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND owner_subject = $2
            RETURNING
                to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS updated_at_text
            "#,
            [
                record.quote_id.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                record.analysis.clone().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    record.updated_at = updated.try_get("", "updated_at_text")?;
    let event = insert_event(&transaction, record).await?;
    transaction.commit().await?;
    Ok(event)
}

pub async fn latest_event(
    database: &DatabaseConnection,
    subject: &str,
    quote_id: Uuid,
) -> Result<QuoteEventRecord, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
                sequence_id,
                quote_id,
                owner_subject,
                status,
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS occurred_at_text
            FROM canonical_quote_event
            WHERE quote_id = $1
              AND owner_subject = $2
            ORDER BY sequence_id DESC
            LIMIT 1
            "#,
            [quote_id.into(), subject.to_owned().into()],
        ))
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    let event = event_from_row(&row)?;
    transaction.commit().await?;
    Ok(event)
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
                id,
                quote_id,
                owner_subject,
                provider,
                model,
                status
            )
            VALUES ($1, $2, $3, 'google-gemini', $4, 'started')
            "#,
            [
                attempt_id.into(),
                record.quote_id.into(),
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
    let attempt_status = if record.status == "completed" {
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
            SET status = $4,
                error_code = $5,
                finished_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND quote_id = $2
              AND owner_subject = $3
              AND status = 'started'
            "#,
            [
                attempt_id.into(),
                record.quote_id.into(),
                record.owner_subject.clone().into(),
                attempt_status.to_owned().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(StoreError::InvalidRecord(
            "canonical_model_attempt transition",
        ));
    }
    transaction.commit().await?;
    Ok(())
}

async fn begin_subject_transaction(
    database: &DatabaseConnection,
    subject: &str,
) -> Result<DatabaseTransaction, StoreError> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(StoreError::Database(DbErr::Custom(
            "canonical quote persistence requires PostgreSQL".into(),
        )));
    }

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

async fn lock_idempotency(
    transaction: &DatabaseTransaction,
    subject: &str,
    idempotency_key: &str,
) -> Result<(), StoreError> {
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [format!("{subject}\0{idempotency_key}").into()],
        ))
        .await?;
    Ok(())
}

struct StoredOperation {
    operation: String,
    quote_id: Uuid,
    request_json: Option<JsonValue>,
}

async fn find_operation(
    transaction: &DatabaseTransaction,
    subject: &str,
    idempotency_key: &str,
) -> Result<Option<StoredOperation>, StoreError> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT operation, quote_id, request_json
            FROM canonical_quote_operation
            WHERE owner_subject = $1
              AND idempotency_key = $2
            "#,
            [subject.to_owned().into(), idempotency_key.to_owned().into()],
        ))
        .await?;
    row.map(|row| {
        Ok(StoredOperation {
            operation: row.try_get("", "operation")?,
            quote_id: row.try_get("", "quote_id")?,
            request_json: row.try_get("", "request_json")?,
        })
    })
    .transpose()
}

async fn insert_operation(
    transaction: &DatabaseTransaction,
    subject: &str,
    idempotency_key: &str,
    operation: &str,
    quote_id: Uuid,
    request_json: Option<JsonValue>,
) -> Result<(), StoreError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_operation (
                owner_subject,
                idempotency_key,
                operation,
                quote_id,
                request_json
            )
            VALUES ($1, $2, $3, $4, $5)
            "#,
            [
                subject.to_owned().into(),
                idempotency_key.to_owned().into(),
                operation.to_owned().into(),
                quote_id.into(),
                request_json.into(),
            ],
        ))
        .await?;
    Ok(())
}

async fn insert_event(
    transaction: &DatabaseTransaction,
    record: &QuoteRecord,
) -> Result<QuoteEventRecord, StoreError> {
    let details = json!({
        "analysis_available": record.analysis.is_some(),
        "error_code": record.error_code.clone(),
    });
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_event (
                quote_id,
                owner_subject,
                status,
                details_json
            )
            VALUES ($1, $2, $3, $4)
            RETURNING
                sequence_id,
                quote_id,
                owner_subject,
                status,
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                    AS occurred_at_text
            "#,
            [
                record.quote_id.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                details.into(),
            ],
        ))
        .await?
        .ok_or(StoreError::InvalidRecord("canonical_quote_event insert"))?;
    event_from_row(&row)
}

async fn get_quote_in_transaction(
    transaction: &DatabaseTransaction,
    subject: &str,
    quote_id: Uuid,
) -> Result<QuoteRecord, StoreError> {
    let row = quote_row(transaction, subject, quote_id, false)
        .await?
        .ok_or(StoreError::QuoteNotFound)?;
    quote_from_row(&row)
}

async fn quote_row(
    transaction: &DatabaseTransaction,
    subject: &str,
    quote_id: Uuid,
    lock: bool,
) -> Result<Option<QueryResult>, StoreError> {
    let lock_clause = if lock { " FOR UPDATE" } else { "" };
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                r#"
                SELECT
                    id,
                    owner_subject,
                    context_record_id,
                    request_json,
                    context_snapshot_markdown,
                    context_snapshot_json,
                    gemini_model,
                    status,
                    analysis_json,
                    error_code,
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS created_at_text,
                    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                        AS updated_at_text
                FROM canonical_quote
                WHERE id = $1
                  AND owner_subject = $2{lock_clause}
                "#
            ),
            [quote_id.into(), subject.to_owned().into()],
        ))
        .await
        .map_err(StoreError::from)
}

fn context_from_row(row: &QueryResult) -> Result<CanonicalContext, StoreError> {
    Ok(CanonicalContext {
        context_json: row.try_get::<JsonValue>("", "context_json")?,
        context_markdown: row.try_get::<String>("", "context_markdown")?,
        id: row.try_get::<Uuid>("", "id")?,
        name: row.try_get::<String>("", "name")?,
    })
}

fn context_snapshot_from_row(row: &QueryResult) -> Result<CanonicalContext, StoreError> {
    Ok(CanonicalContext {
        context_json: row.try_get("", "context_snapshot_json")?,
        context_markdown: row.try_get("", "context_snapshot_markdown")?,
        id: row.try_get("", "context_record_id")?,
        name: "persisted quote context snapshot".into(),
    })
}

fn quote_from_row(row: &QueryResult) -> Result<QuoteRecord, StoreError> {
    let request_json = row.try_get::<JsonValue>("", "request_json")?;
    let request: WireQuoteRequest = serde_json::from_value(request_json)
        .map_err(|_| StoreError::InvalidRecord("canonical_quote.request_json"))?;
    let status = row.try_get::<String>("", "status")?;
    if !matches!(
        status.as_str(),
        "queued" | "analyzing" | "completed" | "failed"
    ) {
        return Err(StoreError::InvalidRecord("canonical_quote.status"));
    }

    Ok(QuoteRecord {
        analysis: row.try_get("", "analysis_json")?,
        context_record_id: row.try_get("", "context_record_id")?,
        error_code: row.try_get("", "error_code")?,
        gemini_model: row.try_get("", "gemini_model")?,
        owner_subject: row.try_get("", "owner_subject")?,
        persistence: "postgres".into(),
        quote_id: row.try_get("", "id")?,
        request,
        status,
        created_at: row.try_get("", "created_at_text")?,
        updated_at: row.try_get("", "updated_at_text")?,
    })
}

fn event_from_row(row: &QueryResult) -> Result<QuoteEventRecord, StoreError> {
    let sequence = row.try_get::<i64>("", "sequence_id")?;
    Ok(QuoteEventRecord {
        owner_subject: row.try_get("", "owner_subject")?,
        quote_id: row.try_get("", "quote_id")?,
        sequence: u64::try_from(sequence)
            .map_err(|_| StoreError::InvalidRecord("canonical_quote_event.sequence_id"))?,
        status: row.try_get("", "status")?,
        occurred_at: row.try_get("", "occurred_at_text")?,
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
            StoreError::IdempotencyKeyReused.code(),
            "idempotency_key_reused"
        );
        assert_eq!(
            StoreError::InvalidRecord("test").code(),
            "invalid_stored_record"
        );
    }
}
