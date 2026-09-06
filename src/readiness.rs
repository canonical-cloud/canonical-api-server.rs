use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::Serialize;
use tracing::error;

const READINESS_SQL: &str = r#"
WITH runtime_role AS (
    SELECT
        rolname = 'canonical_cloud__quote__api_rw'
        AND rolcanlogin
        AND NOT rolsuper
        AND NOT rolcreatedb
        AND NOT rolcreaterole
        AND NOT rolinherit
        AND NOT rolreplication
        AND NOT rolbypassrls
        AND NOT pg_has_role(
            current_user,
            'canonical_cloud__quote__migrator',
            'member'
        ) AS ok
    FROM pg_roles
    WHERE rolname = current_user
),
search_path_contract AS (
    SELECT current_schemas(FALSE) = ARRAY[
        'pg_catalog',
        'canonical_cloud__quote'
    ]::name[] AS ok
),
object_ownership AS (
    SELECT count(*) = 6 AS ok
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relkind IN ('r', 'p', 'S')
      AND pg_get_userbyid(relation.relowner)
          = 'canonical_cloud__quote__migrator'
),
rls_tables AS (
    SELECT count(*) = 5 AS ok
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relname IN (
          'canonical_context',
          'canonical_quote',
          'canonical_quote_operation',
          'canonical_quote_event',
          'canonical_model_attempt'
      )
      AND relation.relkind IN ('r', 'p')
      AND relation.relrowsecurity
      AND relation.relforcerowsecurity
),
owner_policies AS (
    SELECT count(*) = 5 AS ok
    FROM pg_policies
    WHERE schemaname = 'canonical_cloud__quote'
      AND policyname IN (
          'canonical_context_owner_policy',
          'canonical_quote_owner_policy',
          'canonical_quote_operation_owner_policy',
          'canonical_quote_event_owner_policy',
          'canonical_model_attempt_owner_policy'
      )
),
required_constraints AS (
    SELECT count(*) = 17 AS ok
    FROM pg_constraint
    WHERE connamespace = (
        SELECT oid
        FROM pg_namespace
        WHERE nspname = 'canonical_cloud__quote'
    )
      AND conname IN (
          'canonical_context_json_object_check',
          'canonical_context_id_owner_unique',
          'canonical_quote_request_json_object_check',
          'canonical_quote_context_snapshot_json_object_check',
          'canonical_quote_analysis_json_object_check',
          'canonical_quote_id_owner_unique',
          'canonical_quote_context_owner_fk',
          'canonical_quote_operation_owner_key_pk',
          'canonical_quote_operation_key_check',
          'canonical_quote_operation_request_check',
          'canonical_quote_operation_request_json_object_check',
          'canonical_quote_operation_quote_owner_fk',
          'canonical_quote_event_details_json_object_check',
          'canonical_quote_event_quote_owner_fk',
          'canonical_model_attempt_status_finished_check',
          'canonical_model_attempt_time_order_check',
          'canonical_model_attempt_quote_owner_fk'
      )
      AND convalidated
),
required_indexes AS (
    SELECT
        to_regclass(
            'canonical_cloud__quote.canonical_context_owner_active_idx'
        ) IS NOT NULL
        AND to_regclass(
            'canonical_cloud__quote.canonical_context_one_active_per_owner_idx'
        ) IS NOT NULL
        AND to_regclass(
            'canonical_cloud__quote.canonical_quote_owner_created_idx'
        ) IS NOT NULL
        AND to_regclass(
            'canonical_cloud__quote.canonical_quote_operation_quote_created_idx'
        ) IS NOT NULL
        AND to_regclass(
            'canonical_cloud__quote.canonical_quote_event_quote_sequence_idx'
        ) IS NOT NULL
        AND to_regclass(
            'canonical_cloud__quote.canonical_model_attempt_quote_started_idx'
        ) IS NOT NULL AS ok
),
runtime_privileges AS (
    SELECT
        has_schema_privilege(
            current_user,
            'canonical_cloud__quote',
            'USAGE'
        )
        AND NOT has_schema_privilege(
            current_user,
            'canonical_cloud__quote',
            'CREATE'
        )
        AND NOT has_schema_privilege(
            current_user,
            'public',
            'CREATE'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_context',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_context',
            'INSERT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_context',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_context',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_context',
            'TRUNCATE'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote',
            'INSERT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote',
            'TRUNCATE'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_operation',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_operation',
            'INSERT'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_operation',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_operation',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_operation',
            'TRUNCATE'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event',
            'INSERT'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event',
            'TRUNCATE'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_model_attempt',
            'SELECT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_model_attempt',
            'INSERT'
        )
        AND has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_model_attempt',
            'UPDATE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_model_attempt',
            'DELETE'
        )
        AND NOT has_table_privilege(
            current_user,
            'canonical_cloud__quote.canonical_model_attempt',
            'TRUNCATE'
        )
        AND has_sequence_privilege(
            current_user,
            'canonical_cloud__quote.canonical_quote_event_sequence_id_seq',
            'USAGE'
        )
        AND has_function_privilege(
            current_user,
            'canonical_cloud__quote.canonical_set_updated_at()',
            'EXECUTE'
        ) AS ok
)
SELECT
    COALESCE((SELECT ok FROM runtime_role), FALSE)
    AND COALESCE((SELECT ok FROM search_path_contract), FALSE)
    AND COALESCE((SELECT ok FROM object_ownership), FALSE)
    AND COALESCE((SELECT ok FROM rls_tables), FALSE)
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

pub fn router(database: Option<DatabaseConnection>, accepting: Arc<AtomicBool>) -> Router {
    Router::new().route(
        "/readyz",
        get(move || {
            let database = database.clone();
            let accepting = accepting.clone();
            async move {
                if !accepting.load(Ordering::Acquire) {
                    return not_ready("server_draining", "the server is draining");
                }
                readiness(database).await
            }
        }),
    )
}

async fn readiness(database: Option<DatabaseConnection>) -> Response {
    let Some(database) = database else {
        return not_ready(
            "database_not_configured",
            "PostgreSQL is required before the Canonical quote API can receive traffic",
        );
    };

    match check_database(&database).await {
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
                "PostgreSQL is reachable but the quote schema or runtime role is not ready",
            )
        }
    }
}

async fn check_database(database: &DatabaseConnection) -> Result<(), DbErr> {
    if database.get_database_backend() != DatabaseBackend::Postgres {
        return Err(DbErr::Custom(
            "Canonical quote readiness requires PostgreSQL".into(),
        ));
    }

    let row = database
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            READINESS_SQL.to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("readiness query returned no row".into()))?;

    if !row.try_get::<bool>("", "ready")? {
        return Err(DbErr::Custom(
            "quote namespace, role, ownership, RLS, constraints, indexes, or grants are incomplete"
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
    use std::sync::{atomic::AtomicBool, Arc};

    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::{router, READINESS_SQL};

    #[tokio::test]
    async fn ready_route_fails_closed_without_database() {
        let response = router(None, Arc::new(AtomicBool::new(true)))
            .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn readiness_contract_names_every_security_boundary() {
        for required in [
            "canonical_cloud__quote__api_rw",
            "canonical_cloud__quote__migrator",
            "canonical_quote_context_owner_fk",
            "canonical_quote_operation_quote_owner_fk",
            "canonical_quote_operation_owner_policy",
            "canonical_quote_event_quote_owner_fk",
            "canonical_model_attempt_quote_owner_fk",
            "canonical_model_attempt_status_finished_check",
            "canonical_context_one_active_per_owner_idx",
            "rolbypassrls",
            "has_table_privilege",
        ] {
            assert!(READINESS_SQL.contains(required), "{required}");
        }
    }
}
