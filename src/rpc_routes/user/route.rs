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

use super::handlers::{
    self, FindUserByIdOperation, FindUserByIdRequest, FindUsersOperation, FindUsersRequest, UserLookupHeaders,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/find-users", axum_post(post_find_users))
        .route("/v1/find-user-by-id", axum_post(post_find_user_by_id))
}

fn request_headers(headers: &HeaderMap) -> UserLookupHeaders {
    UserLookupHeaders {
        authorization: headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    }
}

#[ores_route(operation = handlers::find_users)]
pub async fn post_find_users(
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

#[ores_route(operation = handlers::find_user_by_id)]
pub async fn post_find_user_by_id(
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
