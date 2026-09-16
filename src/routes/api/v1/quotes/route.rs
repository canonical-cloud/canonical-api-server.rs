use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use canonical_lib::interfaces::QuoteListQuery;
use serde_json::Value as JsonValue;

use crate::AppState;

/// GET /api/v1/quotes -> api-docs operation `list_quotes`.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<QuoteListQuery>,
) -> Response {
    crate::list_quotes(State(state), headers, Query(query))
        .await
        .into_response()
}

/// POST /api/v1/quotes -> api-docs operation `create_quote`.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Response {
    crate::create_quote(State(state), headers, Json(payload))
        .await
        .into_response()
}
