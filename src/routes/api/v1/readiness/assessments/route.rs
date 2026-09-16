use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value as JsonValue;

use crate::AppState;

/// GET /api/v1/readiness/assessments -> `list_readiness_assessments`.
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    crate::list_readiness_assessments(State(state), headers)
        .await
        .into_response()
}

/// POST /api/v1/readiness/assessments -> `create_readiness_assessment`.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<JsonValue>,
) -> Response {
    crate::create_readiness_assessment(State(state), headers, Json(payload))
        .await
        .into_response()
}
