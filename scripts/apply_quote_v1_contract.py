#!/usr/bin/env python3
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace(path: str, old: str, new: str) -> None:
    target = ROOT / path
    text = target.read_text()
    if old not in text:
        raise SystemExit(f"missing expected block in {path}: {old[:80]!r}")
    target.write_text(text.replace(old, new))


# Dependency graph: one immutable contract revision and chrono-backed persisted timestamps.
replace(
    "Cargo.toml",
    "[dependencies]\n",
    "[dependencies]\ncanonical-interfaces = { git = \"https://github.com/canonical-cloud/canonical-interfaces\", rev = \"3415d6d97721b18ee6734659d73a95ff3d35a151\", package = \"canonical-interfaces\" }\nchrono = { version = \"0.4.42\", default-features = false, features = [\"clock\", \"serde\"] }\n",
)
replace(
    "Cargo.toml",
    'features = ["macros", "runtime-tokio-rustls", "sqlx-postgres", "with-json", "with-uuid"]',
    'features = ["macros", "runtime-tokio-rustls", "sqlx-postgres", "with-chrono", "with-json", "with-uuid"]',
)

# Crate modules and imports.
replace("src/lib.rs", "mod gemini;\nmod persistence;", "mod auth;\nmod gemini;\nmod persistence;\nmod wire;")
replace(
    "src/lib.rs",
    "use std::collections::{HashMap, HashSet};\nuse std::env;\nuse std::fmt;\nuse std::sync::Arc;",
    "use std::collections::{HashMap, HashSet};\nuse std::env;\nuse std::fmt;\nuse std::sync::atomic::{AtomicU64, Ordering};\nuse std::sync::Arc;",
)
replace("src/lib.rs", "use subtle::ConstantTimeEq;\n", "")
replace(
    "src/lib.rs",
    'const INTERNAL_TOKEN_HEADER: &str = "x-canonical-internal-token";\nconst SUBJECT_HEADER: &str = "x-canonical-subject";\n',
    "",
)
replace(
    "src/lib.rs",
    'const ALLOWED_FRAMEWORKS: &[&str] = &[\n    "fedramp",\n    "gdpr",\n    "hipaa",\n    "iso-27001",\n    "nist-800-53",\n    "nist-csf",\n    "pci-dss",\n    "soc2",\n];',
    'const ALLOWED_FRAMEWORKS: &[&str] = &[\n    "custom",\n    "fedramp",\n    "gdpr",\n    "hipaa",\n    "iso_27001",\n    "nist_800_53",\n    "nist_csf_2",\n    "pci_dss_4",\n    "soc2_type_1",\n    "soc2_type_2",\n];',
)

# Configuration and authentication verifier.
replace(
    "src/lib.rs",
    "    pub internal_auth_token: String,\n}",
    "    pub internal_auth_token: String,\n    pub shared_auth_verify_url: Option<String>,\n}",
)
replace(
    "src/lib.rs",
    "            internal_auth_token,\n        })",
    "            internal_auth_token,\n            shared_auth_verify_url: env::var(\"SHARED_AUTH_VERIFY_URL\").ok(),\n        })",
)
replace(
    "src/lib.rs",
    '            .field("internal_auth_token", &"[redacted]")\n            .finish()',
    '            .field("internal_auth_token", &"[redacted]")\n            .field("shared_auth_verify_url", &self.shared_auth_verify_url)\n            .finish()',
)
replace(
    "src/lib.rs",
    "    InvalidGeminiModel,\n    MissingInternalAuthToken,",
    "    InvalidGeminiModel,\n    InvalidSharedAuthVerifyUrl,\n    MissingInternalAuthToken,",
)
replace(
    "src/lib.rs",
    '            Self::InvalidGeminiModel => formatter.write_str(\n                "GEMINI_MODEL must contain only ASCII letters, digits, \'.\', \"-\", or \"_\"",\n            ),',
    '            Self::InvalidGeminiModel => formatter.write_str(\n                "GEMINI_MODEL must contain only ASCII letters, digits, \'.\', \"-\", or \"_\"",\n            ),\n            Self::InvalidSharedAuthVerifyUrl => formatter.write_str(\n                "SHARED_AUTH_VERIFY_URL must be the exact HTTPS /shared-auth/auth/verify endpoint, except on loopback or Kubernetes service DNS",\n            ),',
)
# The source uses single-quoted punctuation in the exact display line; apply a fallback insertion.
lib = (ROOT / "src/lib.rs").read_text()
if "Self::InvalidSharedAuthVerifyUrl" not in lib.split("impl fmt::Display", 1)[1]:
    marker = "            Self::MissingInternalAuthToken => {\n"
    if marker not in lib:
        raise SystemExit("missing ConfigError display insertion point")
    lib = lib.replace(marker, "            Self::InvalidSharedAuthVerifyUrl => formatter.write_str(\n                \"SHARED_AUTH_VERIFY_URL must be the exact HTTPS /shared-auth/auth/verify endpoint, except on loopback or Kubernetes service DNS\",\n            ),\n" + marker)
    (ROOT / "src/lib.rs").write_text(lib)

replace(
    "src/lib.rs",
    "    internal_auth_token: Arc<str>,\n    quotes: Arc<RwLock<HashMap<Uuid, QuoteRecord>>>,",
    "    auth: auth::Authenticator,\n    event_sequence: Arc<AtomicU64>,\n    quotes: Arc<RwLock<HashMap<Uuid, QuoteRecord>>>,",
)
replace(
    "src/lib.rs",
    "    pub fn new(\n        internal_auth_token: impl Into<Arc<str>>,\n        gemini_model: impl Into<Arc<str>>,\n        database: Option<DatabaseConnection>,\n    ) -> Self {\n        let (events, _) = broadcast::channel(256);\n        Self {\n            database,\n            events,\n            gemini: None,\n            gemini_model: gemini_model.into(),\n            internal_auth_token: internal_auth_token.into(),\n            quotes: Arc::new(RwLock::new(HashMap::new())),\n        }\n    }",
    "    pub fn new(\n        internal_auth_token: impl Into<Arc<str>>,\n        gemini_model: impl Into<Arc<str>>,\n        database: Option<DatabaseConnection>,\n    ) -> Self {\n        Self::try_new(internal_auth_token, gemini_model, database, None)\n            .expect(\"static test authentication configuration\")\n    }\n\n    pub fn try_new(\n        internal_auth_token: impl Into<Arc<str>>,\n        gemini_model: impl Into<Arc<str>>,\n        database: Option<DatabaseConnection>,\n        shared_auth_verify_url: Option<String>,\n    ) -> Result<Self, ConfigError> {\n        let (events, _) = broadcast::channel(256);\n        Ok(Self {\n            database,\n            events,\n            gemini: None,\n            gemini_model: gemini_model.into(),\n            auth: auth::Authenticator::new(internal_auth_token, shared_auth_verify_url)?,\n            event_sequence: Arc::new(AtomicU64::new(0)),\n            quotes: Arc::new(RwLock::new(HashMap::new())),\n        })\n    }",
)

# Canonical route surface.
replace(
    "src/lib.rs",
    '.route("/v1/quotes", get(list_quotes).post(create_quote))\n        .route("/v1/quotes/{quote_id}", get(get_quote))\n        .route("/v1/quotes/{quote_id}/events", get(quote_events))',
    '.route("/api/v1/quotes", get(list_quotes).post(create_quote))\n        .route("/api/v1/quotes/{quote_id}", get(get_quote))\n        .route("/api/v1/quotes/{quote_id}/retry", axum::routing::post(retry_quote))\n        .route("/api/v1/quotes/{quote_id}/events", get(quote_events))',
)
replace(
    "src/lib.rs",
    '            HeaderName::from_static(INTERNAL_TOKEN_HEADER),',
    '            HeaderName::from_static("x-canonical-internal-token"),',
)

# Generated request and public response boundary.
replace(
    "src/lib.rs",
    "    Json(request): Json<CreateQuoteRequest>,\n) -> Result<impl IntoResponse, ApiError> {\n    let subject = authenticate(&headers, &state.internal_auth_token)?;\n    let request = request.validate_and_normalize()?;",
    "    Json(request): Json<canonical_interfaces::QuoteRequest>,\n) -> Result<impl IntoResponse, ApiError> {\n    let subject = state.auth.authenticate(&headers).await?;\n    let request = wire::normalize_request(request)?;",
)
replace(
    "src/lib.rs",
    "        quote_id,\n        status: \"queued\".into(),\n    };",
    "        quote_id,\n        status: \"queued\".into(),\n        request: request.wire.clone(),\n        created_at: wire::now(),\n        updated_at: wire::now(),\n    };",
)
replace(
    "src/lib.rs",
    "    Ok((StatusCode::ACCEPTED, Json(record)))\n}",
    "    Ok((StatusCode::ACCEPTED, Json(wire::submission(&record))))\n}",
)
replace("src/lib.rs", '            record.status = "completed".into();', '            record.status = "ready".into();')
replace(
    "src/lib.rs",
    "async fn update_quote_state(state: &AppState, record: &QuoteRecord) {\n    state\n        .quotes",
    "async fn update_quote_state(state: &AppState, record: &QuoteRecord) {\n    let mut record = record.clone();\n    record.updated_at = wire::now();\n    state\n        .quotes",
)
replace(
    "src/lib.rs",
    ".insert(record.quote_id, record.clone());\n\n    if let Some(database) = state.database.as_ref() {\n        if let Err(error) = persistence::update_quote(database, record).await {",
    ".insert(record.quote_id, record.clone());\n\n    if let Some(database) = state.database.as_ref() {\n        if let Err(error) = persistence::update_quote(database, &record).await {",
)
replace(
    "src/lib.rs",
    "    publish_event(state, record);\n}",
    "    publish_event(state, &record);\n}",
)
replace(
    "src/lib.rs",
    "fn publish_event(state: &AppState, record: &QuoteRecord) {\n    let _ = state.events.send(QuoteEvent {\n        analysis_available: record.analysis.is_some(),\n        error_code: record.error_code.clone(),\n        owner_subject: record.owner_subject.clone(),\n        quote_id: record.quote_id,\n        status: record.status.clone(),\n    });\n}",
    "fn publish_event(state: &AppState, record: &QuoteRecord) {\n    let sequence = state.event_sequence.fetch_add(1, Ordering::Relaxed) + 1;\n    let _ = state.events.send(QuoteEvent {\n        owner_subject: record.owner_subject.clone(),\n        quote_id: record.quote_id,\n        status: record.status.clone(),\n        sequence,\n        occurred_at: record.updated_at,\n        estimate: wire::detail(record).estimate,\n        problem: wire::detail(record).problem,\n    });\n}",
)
replace(
    "src/lib.rs",
    ") -> Result<Json<Vec<QuoteRecord>>, ApiError> {\n    let subject = authenticate(&headers, &state.internal_auth_token)?;",
    ") -> Result<Json<canonical_interfaces::QuoteListResponse>, ApiError> {\n    let subject = state.auth.authenticate(&headers).await?;",
)
replace("src/lib.rs", "    Ok(Json(records))\n}", "    Ok(Json(wire::list(records)))\n}")
replace(
    "src/lib.rs",
    ") -> Result<Json<QuoteRecord>, ApiError> {\n    let subject = authenticate(&headers, &state.internal_auth_token)?;\n    let record = find_quote(&state, quote_id, &subject).await?;\n    Ok(Json(record))\n}",
    ") -> Result<Json<canonical_interfaces::QuoteDetail>, ApiError> {\n    let subject = state.auth.authenticate(&headers).await?;\n    let record = find_quote(&state, quote_id, &subject).await?;\n    Ok(Json(wire::detail(&record)))\n}\n\nasync fn retry_quote(\n    State(state): State<AppState>,\n    Path(quote_id): Path<Uuid>,\n    headers: HeaderMap,\n) -> Result<(StatusCode, Json<canonical_interfaces::QuoteRetryResponse>), ApiError> {\n    let subject = state.auth.authenticate(&headers).await?;\n    let mut record = find_quote(&state, quote_id, &subject).await?;\n    if record.status != \"failed\" {\n        return Err(ApiError::conflict(\n            \"quote_not_retryable\",\n            \"only failed quotes may be retried\",\n        ));\n    }\n    record.status = \"queued\".into();\n    record.analysis = None;\n    record.error_code = None;\n    record.updated_at = wire::now();\n    update_quote_state(&state, &record).await;\n\n    let request = wire::normalize_request(record.request.clone())?;\n    let context = match state.database.as_ref() {\n        Some(database) => persistence::get_context(\n            database,\n            &subject,\n            record.context_record_id,\n        )\n        .await\n        .map_err(map_store_error)?,\n        None => CanonicalContext::request_only(),\n    };\n    let response = wire::retry(&record);\n    tokio::spawn(run_quote_analysis(state, record, request, context));\n    Ok((StatusCode::ACCEPTED, Json(response)))\n}",
)
replace(
    "src/lib.rs",
    "    let subject = authenticate(&headers, &state.internal_auth_token)?;\n    find_quote(&state, quote_id, &subject).await?;",
    "    let subject = state.auth.authenticate(&headers).await?;\n    find_quote(&state, quote_id, &subject).await?;",
)
replace(
    "src/lib.rs",
    "        if send_json(&mut socket, &record).await.is_err() {",
    "        if send_json(&mut socket, &wire::detail(&record)).await.is_err() {",
)
replace(
    "src/lib.rs",
    "                        if send_json(&mut socket, &event).await.is_err() {",
    "                        if send_json(&mut socket, &wire::event(&event)).await.is_err() {",
)

# Remove the old synchronous auth implementation.
lib = (ROOT / "src/lib.rs").read_text()
start = lib.index("fn authenticate(headers: &HeaderMap")
end = lib.index("#[derive(Clone, Debug, Deserialize, Serialize)]", start)
lib = lib[:start] + lib[end:]
(ROOT / "src/lib.rs").write_text(lib)

# Preserve the server-selected prompt context while storing the canonical wire request.
replace(
    "src/lib.rs",
    "pub struct CreateQuoteRequest {\n",
    "pub struct CreateQuoteRequest {\n    #[serde(skip)]\n    pub(crate) wire: canonical_interfaces::QuoteRequest,\n",
)
replace(
    "src/lib.rs",
    "#[derive(Clone, Debug, Serialize)]\npub struct QuoteRecord {",
    "#[derive(Clone, Debug)]\npub struct QuoteRecord {",
)
replace(
    "src/lib.rs",
    "    pub status: String,\n}",
    "    pub status: String,\n    pub request: canonical_interfaces::QuoteRequest,\n    pub created_at: chrono::DateTime<chrono::Utc>,\n    pub updated_at: chrono::DateTime<chrono::Utc>,\n}",
)
replace(
    "src/lib.rs",
    "struct QuoteEvent {\n    analysis_available: bool,\n    error_code: Option<String>,\n    #[serde(skip_serializing)]\n    owner_subject: String,\n    quote_id: Uuid,\n    status: String,\n}",
    "pub(crate) struct QuoteEvent {\n    pub(crate) owner_subject: String,\n    pub(crate) quote_id: Uuid,\n    pub(crate) status: String,\n    pub(crate) sequence: u64,\n    pub(crate) occurred_at: chrono::DateTime<chrono::Utc>,\n    pub(crate) estimate: Option<canonical_interfaces::QuoteEstimate>,\n    pub(crate) problem: Option<canonical_interfaces::QuoteProblem>,\n}",
)

# Public problem contract includes a requestId and conflict status.
replace(
    "src/lib.rs",
    "struct ErrorBody {\n    code: &'static str,\n    message: &'static str,\n}",
    "#[serde(rename_all = \"camelCase\")]\nstruct ErrorBody {\n    code: &'static str,\n    message: &'static str,\n    request_id: String,\n}",
)
replace(
    "src/lib.rs",
    "    const fn bad_request(code: &'static str, message: &'static str) -> Self {\n        Self {\n            body: ErrorBody { code, message },\n            status: StatusCode::BAD_REQUEST,\n        }\n    }",
    "    fn bad_request(code: &'static str, message: &'static str) -> Self {\n        Self::new(StatusCode::BAD_REQUEST, code, message)\n    }",
)
replace(
    "src/lib.rs",
    "    const fn not_found(code: &'static str, message: &'static str) -> Self {\n        Self {\n            body: ErrorBody { code, message },\n            status: StatusCode::NOT_FOUND,\n        }\n    }",
    "    fn not_found(code: &'static str, message: &'static str) -> Self {\n        Self::new(StatusCode::NOT_FOUND, code, message)\n    }\n\n    fn conflict(code: &'static str, message: &'static str) -> Self {\n        Self::new(StatusCode::CONFLICT, code, message)\n    }",
)
replace(
    "src/lib.rs",
    "    const fn service_unavailable(code: &'static str, message: &'static str) -> Self {\n        Self {\n            body: ErrorBody { code, message },\n            status: StatusCode::SERVICE_UNAVAILABLE,\n        }\n    }",
    "    fn service_unavailable(code: &'static str, message: &'static str) -> Self {\n        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)\n    }",
)
replace(
    "src/lib.rs",
    "    const fn unauthorized(code: &'static str, message: &'static str) -> Self {\n        Self {\n            body: ErrorBody { code, message },\n            status: StatusCode::UNAUTHORIZED,\n        }\n    }",
    "    fn unauthorized(code: &'static str, message: &'static str) -> Self {\n        Self::new(StatusCode::UNAUTHORIZED, code, message)\n    }\n\n    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {\n        Self {\n            body: ErrorBody {\n                code,\n                message,\n                request_id: Uuid::new_v4().to_string(),\n            },\n            status,\n        }\n    }",
)

# Main process passes the exact Shared Auth verify endpoint.
replace(
    "src/main.rs",
    "    let mut state = AppState::new(\n        config.internal_auth_token,\n        config.gemini_model.clone(),\n        database,\n    );",
    "    let mut state = AppState::try_new(\n        config.internal_auth_token,\n        config.gemini_model.clone(),\n        database,\n        config.shared_auth_verify_url,\n    )?;",
)

# Persistence stores the canonical request and timestamps.
replace("src/persistence.rs", "use serde_json::{json, Value as JsonValue};", "use chrono::{DateTime, Utc};\nuse serde_json::{json, Value as JsonValue};")
replace(
    "src/persistence.rs",
    "    let request_json =\n        serde_json::to_value(request).map_err(|_| StoreError::InvalidRecord(\"quote request\"))?;",
    "    let request_json = serde_json::to_value(&request.wire)\n        .map_err(|_| StoreError::InvalidRecord(\"quote request\"))?;",
)
replace(
    "src/persistence.rs",
    "                error_code\n            FROM canonical_quote",
    "                error_code,\n                created_at,\n                updated_at\n            FROM canonical_quote",
)
# Both get and list contain the same selected block.
persistence = (ROOT / "src/persistence.rs").read_text()
if persistence.count("error_code,\n                created_at,\n                updated_at") != 2:
    raise SystemExit("expected get/list timestamp selections")
(ROOT / "src/persistence.rs").write_text(persistence)
replace("src/persistence.rs", '    let attempt_status = if record.status == "completed" {', '    let attempt_status = if record.status == "ready" {')
replace(
    "src/persistence.rs",
    "fn context_from_row(row: &QueryResult) -> Result<CanonicalContext, StoreError> {",
    "pub async fn get_context(\n    database: &DatabaseConnection,\n    subject: &str,\n    context_id: Uuid,\n) -> Result<CanonicalContext, StoreError> {\n    let transaction = begin_subject_transaction(database, subject).await?;\n    let row = transaction\n        .query_one_raw(Statement::from_sql_and_values(\n            DatabaseBackend::Postgres,\n            \"SELECT id, name, context_markdown, context_json FROM canonical_context WHERE id = $1 AND owner_subject = $2 AND active = TRUE\",\n            [context_id.into(), subject.to_owned().into()],\n        ))\n        .await?\n        .ok_or(StoreError::ContextNotFound)?;\n    let context = context_from_row(&row)?;\n    context.validate().map_err(StoreError::InvalidRecord)?;\n    transaction.commit().await?;\n    Ok(context)\n}\n\nfn context_from_row(row: &QueryResult) -> Result<CanonicalContext, StoreError> {",
)
replace(
    "src/persistence.rs",
    "    let request: CreateQuoteRequest = serde_json::from_value(request_json)\n        .map_err(|_| StoreError::InvalidRecord(\"canonical_quote.request_json\"))?;",
    "    let wire_request: canonical_interfaces::QuoteRequest = serde_json::from_value(request_json)\n        .map_err(|_| StoreError::InvalidRecord(\"canonical_quote.request_json\"))?;\n    let request = crate::wire::normalize_request(wire_request.clone())\n        .map_err(|_| StoreError::InvalidRecord(\"canonical_quote.request_json\"))?;",
)
replace(
    "src/persistence.rs",
    '        "queued" | "analyzing" | "completed" | "failed"',
    '        "queued" | "analyzing" | "ready" | "failed"',
)
replace(
    "src/persistence.rs",
    "        status,\n    })",
    "        status,\n        request: wire_request,\n        created_at: row.try_get::<DateTime<Utc>>(\"\", \"created_at\")?,\n        updated_at: row.try_get::<DateTime<Utc>>(\"\", \"updated_at\")?,\n    })",
)

# Tests use canonical routes and payloads.
lib = (ROOT / "src/lib.rs").read_text()
lib = lib.replace('/v1/quotes', '/api/v1/quotes')
lib = lib.replace('body["quote_id"]', 'body["quoteId"]')
lib = lib.replace('quotes[0]["organization_name"]', 'quotes["quotes"][0]["organizationName"]')
old_payload = '''json!({
            "frameworks": ["soc2", "hipaa"],
            "notes": "Initial estimate",
            "organization": {
                "employee_count": 42,
                "industry": "Software",
                "legal_name": "Example Incorporated"
            },
            "target_date": "2027-01-15"
        })'''
new_payload = '''json!({
            "organizationName": "Example Incorporated",
            "contactName": "Casey Example",
            "contactEmail": "casey@example.com",
            "employeeCount": 42,
            "frameworks": ["soc2_type_2", "hipaa"],
            "currentStage": "readiness",
            "infrastructure": ["aws", "saas_only"],
            "dataSensitivity": ["confidential", "pii", "phi"],
            "targetDate": "2027-01-15",
            "hasSecurityProgram": true,
            "hasPolicies": true,
            "hasRiskAssessment": false,
            "hasIncidentResponsePlan": true,
            "hasVendorManagement": false,
            "notes": "Initial estimate",
            "contextKey": "quote-analysis",
            "answersVersion": 1
        })'''
if old_payload not in lib:
    raise SystemExit("missing old test payload")
lib = lib.replace(old_payload, new_payload)
# Remove the obsolete test that mutates fields no longer accepted by deny_unknown_fields.
start = lib.index("    #[test]\n    fn application_context_and_database_context_are_server_selected()")
end = lib.index("    #[tokio::test]\n    async fn quote_history_is_owner_scoped()", start)
lib = lib[:start] + lib[end:]
(ROOT / "src/lib.rs").write_text(lib)

# Gemini unit test constructs the richer internal request.
replace(
    "src/gemini.rs",
    "        let request = CreateQuoteRequest {\n            legacy_context_record_id: None,",
    "        let request = CreateQuoteRequest {\n            wire: canonical_interfaces::QuoteRequest {\n                organization_name: \"Example Incorporated\".into(),\n                contact_name: \"Casey Example\".into(),\n                contact_email: \"casey@example.com\".into(),\n                website: None,\n                employee_count: 42,\n                annual_revenue_band: None,\n                frameworks: vec![\"soc2_type_2\".into(), \"hipaa\".into()],\n                current_stage: \"readiness\".into(),\n                infrastructure: vec![\"aws\".into()],\n                data_sensitivity: vec![\"confidential\".into()],\n                target_date: Some(\"2027-01-15\".into()),\n                has_security_program: true,\n                has_policies: true,\n                has_risk_assessment: false,\n                has_incident_response_plan: true,\n                has_vendor_management: false,\n                notes: Some(\"Initial estimate\".into()),\n                context_key: Some(\"quote-analysis\".into()),\n                answers_version: 1,\n            },\n            legacy_context_record_id: None,",
)
replace("src/gemini.rs", 'frameworks: vec!["soc2".into(), "hipaa".into()],', 'frameworks: vec!["soc2_type_2".into(), "hipaa".into()],')

# Environment and docs expose only names, never values.
env_file = ROOT / ".env.example"
env_text = env_file.read_text()
if "SHARED_AUTH_VERIFY_URL=" not in env_text:
    env_text += "\n# Exact Shared Auth token verification endpoint; required for api.canonical.plus bearer access.\nSHARED_AUTH_VERIFY_URL=https://app.canonical.plus/shared-auth/auth/verify\n"
env_file.write_text(env_text)

print("applied Canonical quote v1 API compatibility patch")
