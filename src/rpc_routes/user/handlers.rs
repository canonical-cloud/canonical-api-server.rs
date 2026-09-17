use canonical_api_server::AppState;
use ores_api_docs::{NoSection, OperationSpec, RpcPayloadCodec, TypedOperationContext};
use ores_api_docs_operation_macros::ores_operation;
use serde::{Deserialize, Serialize};
use shared_auth_client::SharedAuthClient;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct GetUserByIdHeaders {
    pub authorization: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GetUserByIdRequest {
    pub user_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct UserSummary {
    pub id: String,
    pub user_name: Option<String>,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GetUserByIdResponse {
    pub result: UserSummary,
    #[serde(rename = "traceIds")]
    pub trace_ids: Vec<String>,
}

impl GetUserByIdResponse {
    fn new(result: UserSummary, trace_id: &'static str) -> Self {
        Self {
            result,
            trace_ids: vec![trace_id.to_owned()],
        }
    }

    pub(crate) fn prepend_trace_id(&mut self, trace_id: &'static str) {
        self.trace_ids.insert(0, trace_id.to_owned());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GetUserByIdError {
    pub code: String,
    pub message: String,
}

pub(crate) struct GetUserByIdOperation;

impl OperationSpec for GetUserByIdOperation {
    type Path = NoSection;
    type Query = NoSection;
    type RequestHeaders = GetUserByIdHeaders;
    type RequestBody = GetUserByIdRequest;
    type ResponseBody = GetUserByIdResponse;
    type ResponseHeaders = NoSection;
    type ResponseTrailers = NoSection;
    type Error = GetUserByIdError;

    const KEY: &'static str = "canonical_cloud.user.get_user_by_id";
    const CODECS: &'static [RpcPayloadCodec] = &[RpcPayloadCodec::Json];
    const DEFAULT_CODEC: RpcPayloadCodec = RpcPayloadCodec::Json;
}

#[ores_operation(
    spec = GetUserByIdOperation,
    key = "canonical_cloud.user.get_user_by_id",
    codecs("json"),
    default_codec = "json",
    audiences("server"),
    scope = "regular"
)]
pub(crate) async fn get_user_by_id(
    ctx: TypedOperationContext<AppState, GetUserByIdOperation>,
) -> Result<GetUserByIdResponse, GetUserByIdError> {
    let headers = ctx.headers().map_err(|error| rpc_error("headers_missing", error.to_string()))?;
    let body = ctx.body().map_err(|error| rpc_error("body_missing", error.to_string()))?;
    let token = bearer_token(&headers.authorization)?;
    let user_id = body.user_id.trim();
    if user_id.is_empty() || user_id.len() > 256 {
        return Err(rpc_error("invalid_user_id", "user_id must contain 1..=256 characters"));
    }

    let base = std::env::var("SHARED_AUTH_BASE")
        .map_err(|_| rpc_error("directory_not_configured", "SHARED_AUTH_BASE is not configured"))?;
    let client = SharedAuthClient::try_new(base)
        .map_err(|error| rpc_error("directory_not_configured", error.to_string()))?;
    let raw = client
        .scim_get_user(token, user_id)
        .await
        .map_err(|error| rpc_error("user_lookup_failed", error.to_string()))?;

    Ok(GetUserByIdResponse::new(
        UserSummary {
            id: raw
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(user_id)
                .to_owned(),
            user_name: raw
                .get("userName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            display_name: raw
                .get("displayName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            active: raw.get("active").and_then(serde_json::Value::as_bool),
        },
        "ores-trace-canonical-user-handler-b7Qm3vN5rKs",
    ))
}

fn bearer_token(value: &str) -> Result<&str, GetUserByIdError> {
    let token = value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty() && token.len() <= 8192)
        .ok_or_else(|| rpc_error("authorization_required", "Authorization: Bearer <token> is required"))?;
    if token.chars().any(char::is_whitespace) || token.chars().any(char::is_control) {
        return Err(rpc_error("authorization_invalid", "bearer token is malformed"));
    }
    Ok(token)
}

fn rpc_error(code: impl Into<String>, message: impl Into<String>) -> GetUserByIdError {
    GetUserByIdError {
        code: code.into(),
        message: message.into(),
    }
}
