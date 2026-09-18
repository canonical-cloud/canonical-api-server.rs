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

use crate::rpc_routes::user::handlers::{self, FindUserByIdOperation, FindUserByIdRequest};
use crate::rpc_routes::user::http_projection::request_headers;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/v1/find-user-by-id", axum_post(post))
}

#[ores_route(operation = handlers::find_user_by_id)]
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FindUserByIdRequest>,
) -> Response {
    let request = OperationRequestData::new(RpcPayloadCodec::Json);
    request.insert_path::<FindUserByIdOperation>(NoSection);
    request.insert_query::<FindUserByIdOperation>(NoSection);
    request.insert_headers::<FindUserByIdOperation>(request_headers(&headers));
    request.insert_body::<FindUserByIdOperation>(body);
    request.set_semantic_input(serde_json::json!({
        "source": "http",
        "operation": FindUserByIdOperation::KEY,
    }));

    let base = OperationContext::http_with_headers(state, headers);
    let context = TypedOperationContext::<AppState, FindUserByIdOperation>::new(base, request);
    let mut response = match handlers::__ores_invoke_find_user_by_id(context).await {
        Ok(mut output) => {
            output.prepend_trace_id("ores-trace-canonical-user-http-k6Vm3qP9sNx");
            Json(output).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
    response
}
