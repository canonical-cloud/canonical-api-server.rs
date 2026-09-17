use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::post as axum_post,
};
use canonical_api_server::AppState;
use ores_api_docs::{
    NoSection, OperationContext, OperationRequestData, OperationSpec, RpcPayloadCodec,
    TypedOperationContext,
};
use ores_api_docs_operation_macros::ores_route;

use super::handlers::{self, GetUserByIdHeaders, GetUserByIdOperation, GetUserByIdRequest};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/v1/get-user-by-id", axum_post(post))
}

#[ores_route(operation = handlers::get_user_by_id)]
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GetUserByIdRequest>,
) -> Response {
    let request = OperationRequestData::new(RpcPayloadCodec::Json);
    request.insert_path::<GetUserByIdOperation>(NoSection);
    request.insert_query::<GetUserByIdOperation>(NoSection);
    request.insert_headers::<GetUserByIdOperation>(GetUserByIdHeaders {
        authorization: headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    });
    request.insert_body::<GetUserByIdOperation>(body);
    request.set_semantic_input(serde_json::json!({
        "source": "http",
        "operation": GetUserByIdOperation::KEY,
    }));

    let base = OperationContext::http_with_headers(state, headers);
    let context = TypedOperationContext::<AppState, GetUserByIdOperation>::new(base, request);
    let mut response = match handlers::__ores_invoke_get_user_by_id(context).await {
        Ok(mut output) => {
            output.prepend_trace_id("ores-trace-canonical-user-http-k2Vm7qP4sNx");
            Json(output).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
    response
}
