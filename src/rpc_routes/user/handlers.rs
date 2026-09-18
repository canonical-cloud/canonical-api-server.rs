use canonical_api_server::AppState;
use ores_api_docs::{NoSection, OperationSpec, RpcPayloadCodec, TypedOperationContext};
use ores_api_docs_operation_macros::ores_operation;
use serde::{Deserialize, Serialize};
use shared_auth_client::SharedAuthClient;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct UserLookupHeaders {
    pub authorization: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FindUsersRequest {
    pub query: String,
    pub limit: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FindUserByIdRequest {
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
pub(crate) struct FindUsersResponse {
    pub results: Vec<UserSummary>,
    #[serde(rename = "traceIds")]
    pub trace_ids: Vec<String>,
}

impl FindUsersResponse {
    fn new(results: Vec<UserSummary>, trace_id: &'static str) -> Self {
        Self {
            results,
            trace_ids: vec![trace_id.to_owned()],
        }
    }

    pub(crate) fn prepend_trace_id(&mut self, trace_id: &'static str) {
        self.trace_ids.insert(0, trace_id.to_owned());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FindUserByIdResponse {
    pub result: UserSummary,
    #[serde(rename = "traceIds")]
    pub trace_ids: Vec<String>,
}

impl FindUserByIdResponse {
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
pub(crate) struct UserLookupError {
    pub code: String,
    pub message: String,
}

pub(crate) struct FindUsersOperation;

impl OperationSpec for FindUsersOperation {
    type Path = NoSection;
    type Query = NoSection;
    type RequestHeaders = UserLookupHeaders;
    type RequestBody = FindUsersRequest;
    type ResponseBody = FindUsersResponse;
    type ResponseHeaders = NoSection;
    type ResponseTrailers = NoSection;
    type Error = UserLookupError;

    const KEY: &'static str = "canonical_cloud.user.find_users";
    const CODECS: &'static [RpcPayloadCodec] = &[RpcPayloadCodec::Json];
    const DEFAULT_CODEC: RpcPayloadCodec = RpcPayloadCodec::Json;
}

#[ores_operation(
    spec = FindUsersOperation,
    key = "canonical_cloud.user.find_users",
    codecs("json"),
    default_codec = "json",
    audiences("browser", "server"),
    scope = "regular"
)]
pub(crate) async fn find_users(
    ctx: TypedOperationContext<AppState, FindUsersOperation>,
) -> Result<FindUsersResponse, UserLookupError> {
    let headers = ctx
        .headers()
        .map_err(|error| rpc_error("headers_missing", error.to_string()))?;
    let body = ctx
        .body()
        .map_err(|error| rpc_error("body_missing", error.to_string()))?;
    let token = bearer_token(&headers.authorization)?;
    let query = body.query.trim().to_ascii_lowercase();
    if query.is_empty() || query.len() > 256 {
        return Err(rpc_error(
            "invalid_query",
            "query must contain 1..=256 characters",
        ));
    }
    if !(1..=100).contains(&body.limit) {
        return Err(rpc_error(
            "invalid_limit",
            "limit must be between 1 and 100",
        ));
    }

    let raw = directory_client()?
        .scim_list_users(token)
        .await
        .map_err(|_| rpc_error("user_lookup_failed", "user directory lookup failed"))?;
    let resources = raw
        .get("Resources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            rpc_error(
                "directory_response_invalid",
                "user directory response is invalid",
            )
        })?;
    let mut results = Vec::new();
    for value in resources {
        let summary = user_summary(value, None)?;
        let matches = [
            Some(summary.id.as_str()),
            summary.user_name.as_deref(),
            summary.display_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|candidate| candidate.to_ascii_lowercase().contains(&query));
        if matches {
            results.push(summary);
            if results.len() >= usize::from(body.limit) {
                break;
            }
        }
    }
    Ok(FindUsersResponse::new(
        results,
        "ores-trace-canonical-users-handler-f3Qm7vN2rKs",
    ))
}

pub(crate) struct FindUserByIdOperation;

impl OperationSpec for FindUserByIdOperation {
    type Path = NoSection;
    type Query = NoSection;
    type RequestHeaders = UserLookupHeaders;
    type RequestBody = FindUserByIdRequest;
    type ResponseBody = FindUserByIdResponse;
    type ResponseHeaders = NoSection;
    type ResponseTrailers = NoSection;
    type Error = UserLookupError;

    const KEY: &'static str = "canonical_cloud.user.find_user_by_id";
    const CODECS: &'static [RpcPayloadCodec] = &[RpcPayloadCodec::Json];
    const DEFAULT_CODEC: RpcPayloadCodec = RpcPayloadCodec::Json;
}

#[ores_operation(
    spec = FindUserByIdOperation,
    key = "canonical_cloud.user.find_user_by_id",
    codecs("json"),
    default_codec = "json",
    audiences("browser", "server"),
    scope = "regular"
)]
pub(crate) async fn find_user_by_id(
    ctx: TypedOperationContext<AppState, FindUserByIdOperation>,
) -> Result<FindUserByIdResponse, UserLookupError> {
    let headers = ctx
        .headers()
        .map_err(|error| rpc_error("headers_missing", error.to_string()))?;
    let body = ctx
        .body()
        .map_err(|error| rpc_error("body_missing", error.to_string()))?;
    let token = bearer_token(&headers.authorization)?;
    let user_id = body.user_id.trim();
    if user_id.is_empty() || user_id.len() > 256 {
        return Err(rpc_error(
            "invalid_user_id",
            "user_id must contain 1..=256 characters",
        ));
    }

    let raw = directory_client()?
        .scim_get_user(token, user_id)
        .await
        .map_err(|_| rpc_error("user_lookup_failed", "user directory lookup failed"))?;
    Ok(FindUserByIdResponse::new(
        user_summary(&raw, Some(user_id))?,
        "ores-trace-canonical-user-handler-b7Qm3vN5rKs",
    ))
}

fn directory_client() -> Result<SharedAuthClient, UserLookupError> {
    let base = std::env::var("SHARED_AUTH_BASE").map_err(|_| {
        rpc_error(
            "directory_not_configured",
            "SHARED_AUTH_BASE is not configured",
        )
    })?;
    SharedAuthClient::try_new(base)
        .map_err(|error| rpc_error("directory_not_configured", error.to_string()))
}

fn bearer_token(value: &str) -> Result<&str, UserLookupError> {
    let token = value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty() && token.len() <= 8192)
        .ok_or_else(|| {
            rpc_error(
                "authorization_required",
                "Authorization: Bearer <token> is required",
            )
        })?;
    if token.chars().any(char::is_whitespace) || token.chars().any(char::is_control) {
        return Err(rpc_error(
            "authorization_invalid",
            "bearer token is malformed",
        ));
    }
    Ok(token)
}

fn user_summary(
    raw: &serde_json::Value,
    fallback_id: Option<&str>,
) -> Result<UserSummary, UserLookupError> {
    let id = raw
        .get("id")
        .and_then(serde_json::Value::as_str)
        .or(fallback_id)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| rpc_error("directory_response_invalid", "SCIM user is missing id"))?;
    Ok(UserSummary {
        id: id.to_owned(),
        user_name: raw
            .get("userName")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        display_name: raw
            .get("displayName")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        active: raw.get("active").and_then(serde_json::Value::as_bool),
    })
}

fn rpc_error(code: impl Into<String>, message: impl Into<String>) -> UserLookupError {
    UserLookupError {
        code: code.into(),
        message: message.into(),
    }
}
