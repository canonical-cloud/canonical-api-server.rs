//! HTTP projections of this folder's operations, beside the handlers they project.
//!
//! Both are `POST`, and one module cannot hold two `fn post`, so each adapter has
//! its own inline module, with its own `router()`. The bodies are unchanged from
//! `rpc_routes/find_users/route.rs` and `rpc_routes/find_user_by_id/route.rs`,
//! which this file replaces; only the `handlers` / `http_projection` imports
//! became relative.

pub(crate) mod find_users {
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

    use super::super::handlers::{self, FindUsersOperation, FindUsersRequest};
    use super::super::http_projection::request_headers;

    pub(crate) fn router() -> Router<AppState> {
        Router::new().route("/v1/find-users", axum_post(post))
    }

    #[ores_route(operation = handlers::find_users, path = "/v1/find-users")]
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
}

pub(crate) mod find_user_by_id {
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

    use super::super::handlers::{self, FindUserByIdOperation, FindUserByIdRequest};
    use super::super::http_projection::request_headers;

    pub(crate) fn router() -> Router<AppState> {
        Router::new().route("/v1/find-user-by-id", axum_post(post))
    }

    #[ores_route(
        operation = handlers::find_user_by_id,
        path = "/v1/find-user-by-id"
    )]
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
}
