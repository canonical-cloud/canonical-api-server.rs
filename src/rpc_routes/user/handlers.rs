use canonical_api_server::AppState;
use ores_api_docs::{NoSection, OperationSpec, RpcPayloadCodec, TypedOperationContext};
use ores_api_docs_operation_macros::ores_operation;
use serde::{Deserialize, Serialize};
use shared_auth_client::SharedAuthClient;

use crate::telemetry::log_rpc_error;

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
    const ROUTINE_ID: &str = "ores-routine-5a5Cstkakla3OHMjMPKva";

    let headers = ctx.headers().map_err(|error| {
        log_rpc_error(
            FindUsersOperation::KEY,
            "headers_missing",
            "ores-trace-tzj7oJNQM2avNZhKJXVcK",
            ROUTINE_ID,
        );
        rpc_error("headers_missing", error.to_string())
    })?;
    let body = ctx.body().map_err(|error| {
        log_rpc_error(
            FindUsersOperation::KEY,
            "body_missing",
            "ores-trace-BNIOcMqTnLKaHIulhw3sU",
            ROUTINE_ID,
        );
        rpc_error("body_missing", error.to_string())
    })?;
    let token = bearer_token(&headers.authorization).inspect_err(|error| {
        log_rpc_error(
            FindUsersOperation::KEY,
            &error.code,
            "ores-trace-QRF9Bg2esRK-VVeJCNBPh",
            ROUTINE_ID,
        );
    })?;
    let query = body.query.trim().to_ascii_lowercase();
    if query.is_empty() || query.len() > 256 {
        log_rpc_error(
            FindUsersOperation::KEY,
            "invalid_query",
            "ores-trace-V_i5iRASStxbqjIlWv6Ay",
            ROUTINE_ID,
        );
        return Err(rpc_error(
            "invalid_query",
            "query must contain 1..=256 characters",
        ));
    }
    if !(1..=100).contains(&body.limit) {
        log_rpc_error(
            FindUsersOperation::KEY,
            "invalid_limit",
            "ores-trace-P1JOzMmivi4GiQ8oOijNs",
            ROUTINE_ID,
        );
        return Err(rpc_error("invalid_limit", "limit must be between 1 and 100"));
    }

    let raw = directory_client()
        .inspect_err(|error| {
            log_rpc_error(
                FindUsersOperation::KEY,
                &error.code,
                "ores-trace-4sshHDTpvQNXckEtU3uAg",
                ROUTINE_ID,
            );
        })?
        .scim_list_users(token)
        .await
        .map_err(|_| {
            log_rpc_error(
                FindUsersOperation::KEY,
                "user_lookup_failed",
                "ores-trace-O1puWJbp3phvd76AxfuPi",
                ROUTINE_ID,
            );
            rpc_error("user_lookup_failed", "user directory lookup failed")
        })?;
    let resources = raw
        .get("Resources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            log_rpc_error(
                FindUsersOperation::KEY,
                "directory_response_invalid",
                "ores-trace-plBJhDgBC5ULeZvR9XxWR",
                ROUTINE_ID,
            );
            rpc_error(
                "directory_response_invalid",
                "user directory response is invalid",
            )
        })?;
    let mut results = Vec::new();
    for value in resources {
        let summary = user_summary(value, None).inspect_err(|error| {
            log_rpc_error(
                FindUsersOperation::KEY,
                &error.code,
                "ores-trace--93w7g_d7_t5aYxhNoY0N",
                ROUTINE_ID,
            );
        })?;
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
    const ROUTINE_ID: &str = "ores-routine-HJz9D11JRx0hVSzwUFLOr";

    let headers = ctx.headers().map_err(|error| {
        log_rpc_error(
            FindUserByIdOperation::KEY,
            "headers_missing",
            "ores-trace-XN3gidMD78zRwqHAS4itB",
            ROUTINE_ID,
        );
        rpc_error("headers_missing", error.to_string())
    })?;
    let body = ctx.body().map_err(|error| {
        log_rpc_error(
            FindUserByIdOperation::KEY,
            "body_missing",
            "ores-trace-B9q2NtXlcrnBdkfaEkYLY",
            ROUTINE_ID,
        );
        rpc_error("body_missing", error.to_string())
    })?;
    let token = bearer_token(&headers.authorization).inspect_err(|error| {
        log_rpc_error(
            FindUserByIdOperation::KEY,
            &error.code,
            "ores-trace-AHaK7juQYPcAYILHfjraJ",
            ROUTINE_ID,
        );
    })?;
    let user_id = body.user_id.trim();
    if user_id.is_empty() || user_id.len() > 256 {
        log_rpc_error(
            FindUserByIdOperation::KEY,
            "invalid_user_id",
            "ores-trace-X0Il7xHrNBFGTJkyjeUpi",
            ROUTINE_ID,
        );
        return Err(rpc_error(
            "invalid_user_id",
            "user_id must contain 1..=256 characters",
        ));
    }

    let raw = directory_client()
        .inspect_err(|error| {
            log_rpc_error(
                FindUserByIdOperation::KEY,
                &error.code,
                "ores-trace-vfwF7NYajm1zwkp6qdFo4",
                ROUTINE_ID,
            );
        })?
        .scim_get_user(token, user_id)
        .await
        .map_err(|_| {
            log_rpc_error(
                FindUserByIdOperation::KEY,
                "user_lookup_failed",
                "ores-trace-axC5IDkLj9e8XpUFoT6nN",
                ROUTINE_ID,
            );
            rpc_error("user_lookup_failed", "user directory lookup failed")
        })?;
    let summary = user_summary(&raw, Some(user_id)).inspect_err(|error| {
        log_rpc_error(
            FindUserByIdOperation::KEY,
            &error.code,
            "ores-trace-7WyadH5kJXJJ-K0ewD090",
            ROUTINE_ID,
        );
    })?;
    Ok(FindUserByIdResponse::new(
        summary,
        "ores-trace-canonical-user-handler-b7Qm3vN5rKs",
    ))
}

fn directory_client() -> Result<SharedAuthClient, UserLookupError> {
    let base = std::env::var("SHARED_AUTH_BASE")
        .map_err(|_| rpc_error("directory_not_configured", "SHARED_AUTH_BASE is not configured"))?;
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
