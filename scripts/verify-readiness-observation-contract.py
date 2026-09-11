#!/usr/bin/env python3
"""Fail-closed checks for the pinned readiness-observation wire projection."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK_PATH = ROOT / "contracts/readiness-observation.lock.json"
MODULE_PATH = ROOT / "src/readiness_observation_ingest.rs"
WORKFLOW_PATH = ROOT / ".github/workflows/readiness-observation-contract.yml"

EXPECTED_AUTHORITY_COMMIT = "bed2dacd7ccd8242ed4fba4d077b84a01a55e343"
EXPECTED_ADMITTED_HEAD = "c8e4d8dba6c2ac7820aff0bd06b28e27241364b0"
EXPECTED_TJSV_COMMIT = "3171025cbe03a7026a71ce94eea18c910e1431b2"


def fail(message: str) -> None:
    raise SystemExit(message)


lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
module = MODULE_PATH.read_text(encoding="utf-8")
workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
manifest = json.loads((ROOT / "db/namespace.json").read_text(encoding="utf-8"))
schema = (ROOT / "db/schema.sql").read_text(encoding="utf-8")
grants = (ROOT / "db/grants.sql").read_text(encoding="utf-8")

if lock.get("schemaVersion") != 1:
    fail("readiness contract lock schema version drift")
if lock.get("contract") != "canonical.readiness.observation.v1":
    fail("readiness contract identifier drift")
authority = lock.get("authority", {})
if authority != {
    "repository": "canonical-cloud/canonical-interfaces",
    "commit": EXPECTED_AUTHORITY_COMMIT,
    "admittedHead": EXPECTED_ADMITTED_HEAD,
    "typespec": "contracts/readiness-observation/v1/main.tsp",
    "jsonSchema": "contracts/readiness-observation/v1/authored.schema.json",
    "instances": "contracts/readiness-observation/v1/instances",
    "authoritiesPrecedence": "none",
    "editableAuthority": False,
}:
    fail("readiness authority provenance drift")
validator = lock.get("validator", {})
if validator != {
    "repository": "ORESoftware/typespec-json-schema-validator",
    "commit": EXPECTED_TJSV_COMMIT,
    "requiredStatus": "passed",
    "zeroUnexplainedFindings": True,
    "contractIrAdmissible": True,
}:
    fail("TJSV admission provenance drift")
if lock.get("runtimeProjection") != {
    "language": "rust",
    "module": "src/readiness_observation_ingest.rs",
    "transport": "application/json",
    "unknownFields": "deny",
    "initialSubstantiveReview": "unreviewed",
}:
    fail("Rust runtime projection metadata drift")
if lock.get("linear") != "DEN-3938":
    fail("Linear provenance drift")

for required in (
    'const EVENT_SPEC: &str = "canonical.readiness.observation.v1";',
    'const RECEIPT_SPEC: &str = "canonical.readiness.observation.receipt.v1";',
    'const PROBLEM_SPEC: &str = "canonical.readiness.observation.problem.v1";',
    '#[serde(rename_all = "camelCase", deny_unknown_fields)]\nstruct ObservationEvent',
    '#[serde(rename_all = "camelCase", deny_unknown_fields)]\nstruct ObservationAssertion',
    '#[serde(rename_all = "camelCase", deny_unknown_fields)]\nstruct EvidenceItem',
    'spec_version: String',
    'event_id: String',
    'source_id: String',
    'source_sequence: i64',
    'organization: String',
    'observed_at: String',
    'assertions: Vec<ObservationAssertion>',
    'evidence: Vec<EvidenceItem>',
    'framework_id: String',
    'control_id: String',
    'reported_status: ReportedStatus',
    'statement: Option<String>',
    'evidence_ids: Vec<String>',
    'evidence_id: String',
    'kind: String',
    'sha256: String',
    'collected_at: String',
    'locator: Option<String>',
    'content_type: Option<String>',
    'size_bytes: Option<u64>',
    'status: if stored.duplicate',
    'transport_verification: "signature-valid"',
    'substantive_review: "unreviewed"',
):
    if required not in module:
        fail(f"Rust projection omits admitted wire element: {required}")

for value in (
    "Observed",
    "NotObserved",
    "Exception",
    "NotApplicableRequested",
    "Unknown",
):
    if value not in module:
        fail(f"Rust projection omits ReportedStatus value: {value}")

for required in (
    '"/api/v1/readiness/sources/{source_id}/observations"',
    '"/v1/readiness/sources/{source_id}/observations"',
    'header(headers, "x-canonical-webhook-id")',
    'header(headers, "x-canonical-webhook-timestamp")',
    '"x-canonical-webhook-signature"',
    'mac.update(timestamp_text.as_bytes());',
    'mac.update(b".");',
    'mac.update(body);',
    "mac.verify_slice(&supplied)",
    "MAX_CLOCK_SKEW_SECONDS: i64 = 300",
    "MAX_BODY_BYTES: usize = 256 * 1024",
    "source sequence must be the next contiguous value",
    "pg_advisory_xact_lock",
    "allow_memory: false",
):
    if required not in module:
        fail(f"readiness transport/security invariant missing: {required}")

if 'time = { version = "0.3", features = ["formatting", "parsing"] }' not in cargo:
    fail("RFC 3339 parsing feature is not pinned")

if "canonical_readiness_observation" not in schema:
    fail("declarative schema omits readiness observation ledger")
for required in (
    "canonical_readiness_observation_owner_policy",
    "canonical_readiness_observation_owner_source_sequence_unique",
    "canonical_readiness_observation_prior_record_sha256_check",
    "canonical_readiness_observation_record_sha256_check",
    "canonical_readiness_observation_substantive_review_check",
):
    if required not in schema:
        fail(f"readiness ledger constraint missing: {required}")
if "canonical_readiness_observation API privilege contract is not append-only" not in grants:
    fail("append-only runtime grant assertion missing")
if "canonical_readiness_observation" not in manifest["access"]["appendOnlyTables"]:
    fail("readiness ledger is not declared append-only")

for required in (
    EXPECTED_AUTHORITY_COMMIT,
    EXPECTED_TJSV_COMMIT,
    "ORESoftware/typespec-json-schema-validator@",
    "contracts/readiness-observation/v1/main.tsp",
    "contracts/readiness-observation/v1/authored.schema.json",
    "contracts/readiness-observation/v1/instances",
    "persist-credentials: false",
):
    if required not in workflow:
        fail(f"readiness contract workflow omits {required}")

print(
    json.dumps(
        {
            "contract": lock["contract"],
            "authorityCommit": EXPECTED_AUTHORITY_COMMIT,
            "admittedHead": EXPECTED_ADMITTED_HEAD,
            "tjsvCommit": EXPECTED_TJSV_COMMIT,
            "runtimeProjection": str(MODULE_PATH.relative_to(ROOT)),
            "appendOnlyLedger": True,
            "initialSubstantiveReview": "unreviewed",
        },
        sort_keys=True,
    )
)
