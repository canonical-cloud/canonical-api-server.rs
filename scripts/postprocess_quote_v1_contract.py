#!/usr/bin/env python3
"""Finish the deterministic DEN-2642 migration after the structural patch."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

lib = ROOT / "src/lib.rs"
source = lib.read_text()
source, count = re.subn(
    r"#\[derive\(Clone, Debug, Deserialize, Serialize\)\]\n#\[serde\(deny_unknown_fields\)\]\npub struct CreateQuoteRequest \{",
    "#[derive(Clone, Debug)]\npub struct CreateQuoteRequest {",
    source,
    count=1,
)
if count != 1:
    raise SystemExit("normalized request derive block not found exactly once")

start = source.index("pub struct CreateQuoteRequest {")
end = source.index("\n}\n\nimpl CreateQuoteRequest", start)
block = source[start:end]
block, removed = re.subn(r"^    #\[serde\([^\n]*\)\]\n", "", block, flags=re.M)
if removed < 3:
    raise SystemExit("expected at least three stale serde field attributes")
source = source[:start] + block + source[end:]
source = source.replace(
    "    #[serde(skip_serializing)]\n    pub owner_subject:",
    "    pub owner_subject:",
    1,
)
source = source.replace(
    "        build_router, AppState, CreateQuoteRequest, APPLICATION_CONTEXT_MARKDOWN,\n",
    "        build_router, AppState,\n",
    1,
)
source = source.replace("/api/api/v1/quotes", "/api/v1/quotes")
old_list_assertion = (
    "        let quotes = body.as_array().unwrap();\n"
    "        assert_eq!(quotes.len(), 1);\n"
    "        assert_eq!(quotes[\"quotes\"][0][\"organizationName\"], \"Example Incorporated\");\n"
)
new_list_assertion = (
    "        let quotes = body[\"quotes\"].as_array().unwrap();\n"
    "        assert_eq!(quotes.len(), 1);\n"
    "        assert_eq!(quotes[0][\"organizationName\"], \"Example Incorporated\");\n"
)
if old_list_assertion not in source:
    raise SystemExit("owner-scoped list assertion not found")
source = source.replace(old_list_assertion, new_list_assertion, 1)
lib.write_text(source)

wire = ROOT / "src/wire.rs"
source = wire.read_text()
old = "    let mut request = CreateQuoteRequest {\n"
if old not in source:
    raise SystemExit("wire request construction not found")
source = source.replace(old, "    let request = CreateQuoteRequest {\n", 1)
old = "    request.validate_and_normalize()?;\n    Ok(request)\n"
if old not in source:
    raise SystemExit("wire request validation return not found")
source = source.replace(old, "    request.validate_and_normalize()\n", 1)
wire.write_text(source)

gemini = ROOT / "src/gemini.rs"
source = gemini.read_text()
old = "serde_json::to_string_pretty(request)"
if old not in source:
    raise SystemExit("Gemini request serialization boundary not found")
source = source.replace(old, "serde_json::to_string_pretty(&request.wire)", 1)
gemini.write_text(source)

print("postprocessed Canonical quote v1 API compatibility patch")
