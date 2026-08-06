use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult,
    Statement, TransactionTrait,
};
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use uuid::Uuid;

use crate::{CanonicalContext, CreateQuoteRequest, QuoteRecord};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("canonical context was not found")]
    ContextNotFound,
    #[error("database operation failed")]
    Database(#[from] DbErr),
    #[error("stored record is invalid: {0}")]
    InvalidRecord(&'static str),
    #[error("quote was not found")]
    QuoteNotFound,
}

impl StoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ContextNotFound => "context_not_found",
            Self::Database(_) => "database_error",
            Self::InvalidRecord(_) => "invalid_stored_record",
            Self::QuoteNotFound => "quote_not_found",
        }
    }
}

pub async fn create_quote(
    database: &DatabaseConnection,
    subject: &str,
    request: &CreateQuoteRequest,
    record: &QuoteRecord,
) -> Result<CanonicalContext, StoreError> {
    let transaction = begin_subject_transaction(database, subject).await?;

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

    let request_json =
        serde_json::to_value(request).map_err(|_| StoreError::InvalidRecord("quote request"))?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
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
            "#,
            [
                record.quote_id.into(),
                subject.to_owned().into(),
                context.id.into(),
                request_json.into(),
                request.markdown_context.clone().into(),
                context.context_markdown.clone().into(),
                context.context_json.clone().into(),
                record.gemini_model.clone().into(),
                record.status.clone().into(),
            ],
        ))
        .await?;

    insert_event(&transaction, record).await?;
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
            r#"
            SELECT
                id,
                owner_subject,
                context_record_id,
                request_json,
                gemini_model,
                status,
                analysis_json,
                error_code
            FROM canonical_quote
            WHERE id = $1
              AND owner_subject = $2
            "#,
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
            r#"
            SELECT
                id,
                owner_subject,
                context_record_id,
                request_json,
                gemini_model,
                status,
                analysis_json,
                error_code
            FROM canonical_quote
            WHERE owner_subject = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
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
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE canonical_quote
            SET status = $3,
                analysis_json = $4,
                error_code = $5,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND owner_subject = $2
            "#,
            [
                record.quote_id.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                record.analysis.clone().into(),
                record.error_code.clone().into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
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

async fn insert_event(
    transaction: &DatabaseTransaction,
    record: &QuoteRecord,
) -> Result<(), StoreError> {
    let details = json!({
        "analysis_available": record.analysis.is_some(),
        "error_code": record.error_code.clone(),
    });
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_quote_event (
                quote_id,
                owner_subject,
                status,
                details_json
            )
            VALUES ($1, $2, $3, $4)
            "#,
            [
                record.quote_id.into(),
                record.owner_subject.clone().into(),
                record.status.clone().into(),
                details.into(),
            ],
        ))
        .await?;
    Ok(())
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
    let request: CreateQuoteRequest = serde_json::from_value(request_json)
        .map_err(|_| StoreError::InvalidRecord("canonical_quote.request_json"))?;
    let status = row.try_get::<String>("", "status")?;
    if !matches!(
        status.as_str(),
        "queued" | "analyzing" | "completed" | "failed"
    ) {
        return Err(StoreError::InvalidRecord("canonical_quote.status"));
    }

    Ok(QuoteRecord {
        analysis: row.try_get::<Option<JsonValue>>("", "analysis_json")?,
        context_record_id: row.try_get::<Uuid>("", "context_record_id")?,
        error_code: row.try_get::<Option<String>>("", "error_code")?,
        frameworks: request.frameworks,
        gemini_model: row.try_get::<String>("", "gemini_model")?,
        organization_name: request.organization.legal_name,
        owner_subject: row.try_get::<String>("", "owner_subject")?,
        persistence: "postgres".into(),
        quote_id: row.try_get::<Uuid>("", "id")?,
        status,
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
            StoreError::InvalidRecord("test").code(),
            "invalid_stored_record"
        );
    }
}
