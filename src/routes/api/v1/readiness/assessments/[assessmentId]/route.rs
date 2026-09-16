use axum::{extract::{Request, State}, response::Response};

pub async fn get(
    State(state): State<super::FilesystemRouteState>,
    request: Request,
) -> Response {
    super::forward_filesystem_request(state, request).await
}
