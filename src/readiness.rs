use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use canonical_orm_core::QuoteStore;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::Serialize;
use tracing::error;

/// Readiness of the API-local append-only observation ledger. The quote tables,
/// runtime role and search_path are verified by `QuoteStore::readiness` in
/// canonical-orm-core; this query covers only what the API still owns directly.
const OBSERVATION_READINESS_SQL: &str = r#"
WITH observation_table AS (
    SELECT count(*) = 1 AS ok
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relname = 'canonical_readiness_observation'
      AND relation.relkind IN ('r', 'p')
      AND relation.relrowsecurity
      AND relation.relforcerowsecurity
      AND pg_get_userbyid(relation.relowner)
          = 'canonical_cloud__quote__migrator'
),
owner_policies AS (
    SELECT count(*) = 1 AS ok
    FROM pg_policies
    WHERE schemaname = 'canonical_cloud__quote'
      AND tablename = 'canonical_readiness_observation'
      AND policyname = 'canonical_readiness_observation_owner_policy'
),
required_constraints AS (
    SELECT count(*) = 15 AS ok
    FROM pg_constraint
    WHERE connamespace = (
        SELECT oid
        FROM pg_namespace
        WHERE nspname = 'canonical_cloud__quote'
    )
      AND conname IN (
          'canonical_readiness_observation_owner_source_event_pk',
          'canonical_readiness_observation_owner_source_sequence_unique',
          'canonical_readiness_observation_receipt_id_unique',
          'canonical_readiness_observation_source_id_check',
          'canonical_readiness_observation_event_id_check',
          'canonical_readiness_observation_source_sequence_check',
          'canonical_readiness_observation_payload_sha256_check',
          'canonical_readiness_observation_prior_record_sha256_check',
          'canonical_readiness_observation_record_sha256_check',
          'canonical_readiness_observation_receipt_id_check',
          'canonical_readiness_observation_key_id_check',
          'canonical_readiness_observation_event_json_object_check',
          'canonical_readiness_observation_raw_body_octets_check',
          'canonical_readiness_observation_transport_verification_check',
          'canonical_readiness_observation_substantive_review_check'
      )
      AND convalidated
),
required_indexes AS (
    SELECT to_regclass(
        'canonical_cloud__quote.canonical_readiness_observation_owner_received_idx'
    ) IS NOT NULL AS ok
),
runtime_privileges AS (
    SELECT
        has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_readiness_observation',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_readiness_observation',
            'INSERT'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_readiness_observation',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_readiness_observation',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_readiness_observation',
            'TRUNCATE'
        ) AS ok
)
SELECT
    COALESCE((SELECT ok FROM observation_table), FALSE)
    AND COALESCE((SELECT ok FROM owner_policies), FALSE)
    AND COALESCE((SELECT ok FROM required_constraints), FALSE)
    AND COALESCE((SELECT ok FROM required_indexes), FALSE)
    AND COALESCE((SELECT ok FROM runtime_privileges), FALSE) AS ready
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyResponse {
    database_ready: bool,
    service: &'static str,
    status: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct NotReadyResponse {
    code: &'static str,
    message: &'static str,
}

/// Quote store (canonical-orm-core) plus the API-local observation ledger pool.
pub type ReadinessDatabase = (QuoteStore, DatabaseConnection);

pub fn router(database: Option<ReadinessDatabase>) -> Router {
    Router::new().route(
        "/readyz",
        get(move || {
            let database = database.clone();
            async move { readiness(database).await }
        }),
    )
}

async fn readiness(database: Option<ReadinessDatabase>) -> Response {
    let Some((quotes, observations)) = database else {
        return not_ready(
            "database_not_configured",
            "PostgreSQL is required before the Canonical quote API can receive traffic",
        );
    };

    let checked = match quotes.readiness().await {
        Ok(()) => check_observation_ledger(&observations).await,
        Err(error) => Err(error),
    };

    match checked {
        Ok(()) => Json(ReadyResponse {
            database_ready: true,
            service: "canonical-api-server",
            status: "ready",
            version: env!("CARGO_PKG_VERSION"),
        })
        .into_response(),
        Err(error) => {
            error!(
                error_code = "database_not_ready",
                %error,
                "PostgreSQL readiness check failed"
            );
            not_ready(
                "database_not_ready",
                "PostgreSQL is reachable but the quote/readiness schema or runtime role is not ready",
            )
        }
    }
}

async fn check_observation_ledger(database: &DatabaseConnection) -> Result<(), DbErr> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(DbErr::Custom(
            "Canonical readiness observation ledger requires PostgreSQL".into(),
        ));
    }

    let row = database
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            OBSERVATION_READINESS_SQL.to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("readiness query returned no row".into()))?;

    if !row.try_get::<bool>("", "ready")? {
        return Err(DbErr::Custom(
            "readiness observation ledger RLS, constraints, indexes, or grants are incomplete"
                .into(),
        ));
    }

    Ok(())
}

fn not_ready(code: &'static str, message: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(NotReadyResponse { code, message }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::OBSERVATION_READINESS_SQL;

    #[test]
    fn observation_readiness_names_every_security_boundary() {
        for required in [
            "canonical_readiness_observation_owner_policy",
            "canonical_readiness_observation_owner_received_idx",
            "canonical_readiness_observation_substantive_review_check",
            "canonical_cloud__quote__migrator",
            "relforcerowsecurity",
            "has_table_privilege",
        ] {
            assert!(OBSERVATION_READINESS_SQL.contains(required), "{required}");
        }
    }
}
