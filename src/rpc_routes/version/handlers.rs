use canonical_api_server::AppState;
use ores_api_docs::{NoSection, OperationSpec, RpcPayloadCodec, TypedOperationContext};
use ores_api_docs_operation_macros::ores_operation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct VersionResult {
    pub service: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct OperationEnvelope<T> {
    pub result: T,
    #[serde(rename = "traceIds")]
    pub trace_ids: Vec<String>,
}

impl<T> OperationEnvelope<T> {
    fn new(result: T, trace_id: &'static str) -> Self {
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
pub(crate) struct VersionError {
    pub code: String,
    pub message: String,
}

pub(crate) struct VersionOperation;

impl OperationSpec for VersionOperation {
    type Path = NoSection;
    type Query = NoSection;
    type RequestHeaders = NoSection;
    type RequestBody = NoSection;
    type ResponseBody = OperationEnvelope<VersionResult>;
    type ResponseHeaders = NoSection;
    type ResponseTrailers = NoSection;
    type Error = VersionError;

    const KEY: &'static str = "canonical_cloud.version.get_version";
    const CODECS: &'static [RpcPayloadCodec] = &[RpcPayloadCodec::Json];
    const DEFAULT_CODEC: RpcPayloadCodec = RpcPayloadCodec::Json;
}

#[ores_operation(
    spec = VersionOperation,
    key = "canonical_cloud.version.get_version",
    codecs("json"),
    default_codec = "json",
    audiences("browser", "server"),
    scope = "regular"
)]
pub(crate) async fn get_version(
    _ctx: TypedOperationContext<AppState, VersionOperation>,
) -> Result<OperationEnvelope<VersionResult>, VersionError> {
    Ok(OperationEnvelope::new(
        VersionResult {
            service: "canonical-api-server".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        "ores-trace-canonical-version-handler-k8Qm4vN2rTx",
    ))
}
