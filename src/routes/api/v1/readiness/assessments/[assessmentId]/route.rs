use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::AppState;

/// GET /api/v1/readiness/assessments/{assessmentId}
/// -> `get_readiness_assessment`.
pub async fn get(
    State(state): State<AppState>,
    Path(assessment_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    crate::get_readiness_assessment(State(state), Path(assessment_id), headers)
        .await
        .into_response()
}
