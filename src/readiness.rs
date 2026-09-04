use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use canonical_orm_core::QuoteStore;
use serde::Serialize;
use tracing::error;

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

pub fn router(database: Option<QuoteStore>) -> Router {
    Router::new().route(
        "/readyz",
        get(move || {
            let database = database.clone();
            async move { readiness(database).await }
        }),
    )
}

async fn readiness(database: Option<QuoteStore>) -> Response {
    let Some(database) = database else {
        return not_ready(
            "database_not_configured",
            "PostgreSQL is required before the Canonical quote API can receive traffic",
        );
    };

    match database.readiness().await {
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

fn not_ready(code: &'static str, message: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(NotReadyResponse { code, message }),
    )
        .into_response()
}
