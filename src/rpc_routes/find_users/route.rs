use axum::{
    extract::State,
    http::{header::CACHE_CONTROL, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post as axum_post,
    Json, Router,
};
use canonical_api_server::AppState;
use ores_api_docs::{
    NoSection, OperationContext, OperationRequestData, OperationSpec, RpcPayloadCodec,
    TypedOperationContext,
};
use ores_api_docs_operation_macros::ores_route;

use crate::rpc_routes::user::handlers::{self, FindUsersOperation, FindUsersRequest};
use crate::rpc_routes::user::http_projection::request_headers;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/v1/find-users", axum_post(post))
}

#[ores_route(operation = handlers::find_users)]
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FindUsersRequest>,
) -> Response {
    let request = OperationRequestData::new(RpcPayloadCodec::Json);
    request.insert_path::<FindUsersOperation>(NoSection);
    request.insert_query::<FindUsersOperation>(NoSection);
    request.insert_headers::<FindUsersOperation>(request_headers(&headers));
    request.insert_body::<FindUsersOperation>(body);
    request.set_semantic_input(serde_json::json!({
        "source": "http",
        "operation": FindUsersOperation::KEY,
    }));

    let base = OperationContext::http_with_headers(state, headers);
    let context = TypedOperationContext::<AppState, FindUsersOperation>::new(base, request);
    let mut response = match handlers::__ores_invoke_find_users(context).await {
        Ok(mut output) => {
            output.prepend_trace_id("ores-trace-canonical-users-http-k5Vm2qP8sNx");
            Json(output).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
    response
}
