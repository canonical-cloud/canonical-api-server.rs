use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::get,
};
use canonical_api_server::AppState;
use ores_api_docs::{
    NoSection, OperationContext, OperationRequestData, OperationSpec, RpcPayloadCodec,
    TypedOperationContext,
};
use ores_api_docs_operation_macros::ores_route;

use super::handlers::{self, VersionOperation};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/v1/version", get(get))
}

#[ores_route(operation = handlers::get_version)]
pub async fn get(State(state): State<AppState>) -> Response {
    let request = OperationRequestData::new(RpcPayloadCodec::Json);
    request.insert_path::<VersionOperation>(NoSection);
    request.insert_query::<VersionOperation>(NoSection);
    request.insert_headers::<VersionOperation>(NoSection);
    request.insert_body::<VersionOperation>(NoSection);
    request.set_semantic_input(serde_json::json!({
        "source": "http",
        "operation": VersionOperation::KEY,
    }));

    let context = TypedOperationContext::<AppState, VersionOperation>::new(
        OperationContext::http(state),
        request,
    );

    let mut response = match handlers::__ores_invoke_get_version(context).await {
        Ok(mut output) => {
            output.prepend_trace_id("ores-trace-canonical-version-http-p7Xn2mR9vQs");
            Json(output).into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
