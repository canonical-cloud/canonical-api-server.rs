#!/usr/bin/env python3
"""Fail closed when the Rust readiness ingress drifts from the admitted peer authorities."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / ".contracts/canonical-interfaces/contracts/readiness-observation/v1/authored.schema.json"
RUST = ROOT / "src/readiness_observation_ingest.rs"


def fail(message: str) -> None:
    raise SystemExit(message)


schema = json.loads(CONTRACT.read_text(encoding="utf-8"))
rust = RUST.read_text(encoding="utf-8")
defs = schema.get("$defs", {})

expected_event_fields = {
    "specVersion",
    "eventId",
    "sourceId",
    "sourceSequence",
    "organization",
    "observedAt",
    "assertions",
    "evidence",
}
actual_required = set(defs["ObservationEvent"]["required"])
if actual_required != expected_event_fields:
    fail(f"ObservationEvent required-field drift: {sorted(actual_required)}")

expected_statuses = {
    "observed",
    "not-observed",
    "exception",
    "not-applicable-requested",
    "unknown",
}
actual_statuses = set(defs["ReportedStatus"]["enum"])
if actual_statuses != expected_statuses:
    fail(f"ReportedStatus drift: {sorted(actual_statuses)}")

expected_receipt_fields = {
    "specVersion",
    "receiptId",
    "eventId",
    "sourceId",
    "sourceSequence",
    "status",
    "duplicate",
    "payloadSha256",
    "receivedAt",
    "transportVerification",
    "substantiveReview",
}
actual_receipt = set(defs["ObservationReceipt"]["required"])
if actual_receipt != expected_receipt_fields:
    fail(f"ObservationReceipt required-field drift: {sorted(actual_receipt)}")

for token in (
    'const CONTRACT_VERSION: &str = "canonical.readiness.observation.v1";',
    'const RECEIPT_VERSION: &str = "canonical.readiness.observation.receipt.v1";',
    '#[serde(rename_all = "camelCase", deny_unknown_fields)]',
    "struct ObservationEvent",
    "spec_version: String",
    "event_id: String",
    "source_id: String",
    "source_sequence: u64",
    "organization: String",
    "observed_at: String",
    "assertions: Vec<ObservationAssertion>",
    "evidence: Vec<EvidenceItem>",
    "enum ReportedStatus",
    "NotObserved",
    "NotApplicableRequested",
    "Unknown",
    "struct ObservationReceipt",
    "transport_verification: TransportVerification",
    "substantive_review: SubstantiveReview",
):
    if token not in rust:
        fail(f"Rust binding omits admitted contract token: {token}")

for prohibited in (
    "accepted-as-evidence: true",
    "certified: true",
    "compliant: true",
):
    if prohibited in rust.lower():
        fail(f"Rust binding contains prohibited assurance shortcut: {prohibited}")

print(
    json.dumps(
        {
            "contract": "canonical.readiness.observation.v1",
            "event_fields": sorted(expected_event_fields),
            "receipt_fields": sorted(expected_receipt_fields),
            "reported_statuses": sorted(expected_statuses),
            "rust_binding": str(RUST.relative_to(ROOT)),
            "tjsv_authorities": "peer",
        },
        sort_keys=True,
    )
)
