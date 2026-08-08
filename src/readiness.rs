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
        NOT rolsuper
        AND NOT rolcreaterole
        AND NOT rolcreatedb
        AND NOT rolreplication
        AND NOT rolbypassrls AS ok
    FROM pg_roles
    WHERE rolname = current_user
),
rls_tables AS (
    SELECT count(*) = 4 AS ok
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'public'
      AND relation.relname IN (
          'canonical_context',
          'canonical_quote',
          'canonical_quote_event',
          'canonical_model_attempt'
      )
      AND relation.relkind IN ('r', 'p')
      AND relation.relrowsecurity
      AND relation.relforcerowsecurity
),
owner_policies AS (
    SELECT count(*) = 4 AS ok
    FROM pg_policies
    WHERE schemaname = 'public'
      AND policyname IN (
          'canonical_context_owner_policy',
          'canonical_quote_owner_policy',
          'canonical_quote_event_owner_policy',
          'canonical_model_attempt_owner_policy'
      )
),
owner_constraints AS (
    SELECT count(*) = 3 AS ok
    FROM pg_constraint
    WHERE conname IN (
        'canonical_quote_context_owner_fk',
        'canonical_quote_event_quote_owner_fk',
        'canonical_model_attempt_quote_owner_fk'
    )
      AND convalidated
),
required_indexes AS (
    SELECT
        to_regclass('public.canonical_context_one_active_per_owner_idx') IS NOT NULL
        AND to_regclass('public.canonical_quote_owner_created_idx') IS NOT NULL
        AND to_regclass('public.canonical_quote_event_quote_sequence_idx') IS NOT NULL
        AND to_regclass('public.canonical_model_attempt_quote_started_idx') IS NOT NULL AS ok
),
runtime_privileges AS (
    SELECT
        has_table_privilege(current_user, 'public.canonical_context', 'SELECT')
        AND has_table_privilege(current_user, 'public.canonical_context', 'INSERT')
        AND has_table_privilege(current_user, 'public.canonical_context', 'UPDATE')
        AND NOT has_table_privilege(current_user, 'public.canonical_context', 'DELETE')
        AND NOT has_table_privilege(current_user, 'public.canonical_context', 'TRUNCATE')
        AND has_table_privilege(current_user, 'public.canonical_quote', 'SELECT')
        AND has_table_privilege(current_user, 'public.canonical_quote', 'INSERT')
        AND has_table_privilege(current_user, 'public.canonical_quote', 'UPDATE')
        AND NOT has_table_privilege(current_user, 'public.canonical_quote', 'DELETE')
        AND NOT has_table_privilege(current_user, 'public.canonical_quote', 'TRUNCATE')
        AND has_table_privilege(current_user, 'public.canonical_quote_event', 'SELECT')
        AND has_table_privilege(current_user, 'public.canonical_quote_event', 'INSERT')
        AND NOT has_table_privilege(current_user, 'public.canonical_quote_event', 'UPDATE')
        AND NOT has_table_privilege(current_user, 'public.canonical_quote_event', 'DELETE')
        AND NOT has_table_privilege(current_user, 'public.canonical_quote_event', 'TRUNCATE')
        AND has_table_privilege(current_user, 'public.canonical_model_attempt', 'SELECT')
        AND has_table_privilege(current_user, 'public.canonical_model_attempt', 'INSERT')
        AND has_table_privilege(current_user, 'public.canonical_model_attempt', 'UPDATE')
        AND NOT has_table_privilege(current_user, 'public.canonical_model_attempt', 'DELETE')
        AND NOT has_table_privilege(current_user, 'public.canonical_model_attempt', 'TRUNCATE')
        AND has_sequence_privilege(
            current_user,
            'public.canonical_quote_event_sequence_id_seq',
            'USAGE'
        ) AS ok
)
SELECT
    COALESCE((SELECT ok FROM runtime_role), FALSE)
    AND COALESCE((SELECT ok FROM rls_tables), FALSE)
    AND COALESCE((SELECT ok FROM owner_policies), FALSE)
    AND COALESCE((SELECT ok FROM owner_constraints), FALSE)
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

pub fn router(database: Option<DatabaseConnection>) -> Router {
    Router::new().route(
        "/readyz",
        get(move || {
            let database = database.clone();
            async move { readiness(database).await }
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
            error!(error_code = "database_not_ready", %error, "PostgreSQL readiness check failed");
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
    let ready = row.try_get::<bool>("", "ready")?;
    if !ready {
        return Err(DbErr::Custom(
            "quote schema, RLS, owner constraints, indexes, or runtime grants are incomplete"
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
