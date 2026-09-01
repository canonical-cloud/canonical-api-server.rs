use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement,
    TransactionTrait,
};
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use super::{model::AcceptedRegistration, ApiError};

pub(super) async fn persist(
    database: &DatabaseConnection,
    accepted: &AcceptedRegistration,
) -> Result<(), ApiError> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(ApiError::Unavailable);
    }
    let transaction = database.begin().await.map_err(|_| ApiError::Unavailable)?;
    transaction
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT set_config('canonical.pre_interest_write', '1', true)".to_owned(),
        ))
        .await
        .map_err(|_| ApiError::Unavailable)?;

    let registration_id = upsert_registration(&transaction, accepted).await?;
    let consent_id = insert_consent(&transaction, registration_id, accepted).await?;
    if let Some(consent_id) = consent_id {
        insert_outbox(&transaction, registration_id, consent_id, accepted).await?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::Unavailable)?;
    Ok(())
}

async fn upsert_registration<C>(
    database: &C,
    accepted: &AcceptedRegistration,
) -> Result<Uuid, ApiError>
where
    C: ConnectionTrait,
{
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_cloud__quote.canonical_pre_interest_registration (
                id,
                normalized_email,
                email_alias,
                party_type,
                organization_name,
                first_source_host
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (email_alias, party_type)
            DO UPDATE SET last_seen_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
            [
                Uuid::new_v4().into(),
                accepted.email.clone().into(),
                accepted.email_alias.clone().into(),
                accepted.party_type.as_str().to_owned().into(),
                accepted.organization_name.clone().into(),
                accepted.source_host.clone().into(),
            ],
        ))
        .await
        .map_err(|_| ApiError::Unavailable)?
        .ok_or(ApiError::Unavailable)?;
    row.try_get("", "id").map_err(|_| ApiError::Unavailable)
}

async fn insert_consent<C>(
    database: &C,
    registration_id: Uuid,
    accepted: &AcceptedRegistration,
) -> Result<Option<Uuid>, ApiError>
where
    C: ConnectionTrait,
{
    let interest_areas = serde_json::to_value(&accepted.interest_areas)
        .map_err(|_| ApiError::Unavailable)?;
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_cloud__quote.canonical_pre_interest_consent (
                id,
                registration_id,
                request_alias,
                consent_version,
                interest_areas,
                organization_name,
                source_host,
                locale,
                referral_code
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (request_alias) DO NOTHING
            RETURNING id
            "#,
            [
                Uuid::new_v4().into(),
                registration_id.into(),
                accepted.request_alias.clone().into(),
                accepted.consent_version.clone().into(),
                interest_areas.into(),
                accepted.organization_name.clone().into(),
                accepted.source_host.clone().into(),
                accepted.locale.clone().into(),
                accepted.referral_code.clone().into(),
            ],
        ))
        .await
        .map_err(|_| ApiError::Unavailable)?;
    row.map(|row: QueryResult| row.try_get("", "id").map_err(|_| ApiError::Unavailable))
        .transpose()
}

async fn insert_outbox<C>(
    database: &C,
    registration_id: Uuid,
    consent_id: Uuid,
    accepted: &AcceptedRegistration,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    let payload: JsonValue = json!({
        "registrationId": registration_id,
        "consentId": consent_id,
        "partyType": accepted.party_type,
        "sourceHost": accepted.source_host,
    });
    database
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO canonical_cloud__quote.canonical_pre_interest_outbox (
                id,
                registration_id,
                consent_id,
                event_type,
                payload_json
            )
            VALUES ($1, $2, $3, 'pre_interest.accepted.v1', $4)
            "#,
            [
                Uuid::new_v4().into(),
                registration_id.into(),
                consent_id.into(),
                payload.into(),
            ],
        ))
        .await
        .map_err(|_| ApiError::Unavailable)?;
    Ok(())
}
