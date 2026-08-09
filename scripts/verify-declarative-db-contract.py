#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = "canonical_cloud__quote"
ROLES = {
    "migrator": "canonical_cloud__quote__migrator",
    "apiRuntime": "canonical_cloud__quote__api_rw",
    "webRuntime": "canonical_cloud__quote__web_ro",
}
DPM_REVISION = "d05a7880987ddaa271fa88b52c787390ef12b899"


def fail(message: str) -> None:
    raise SystemExit(message)


manifest = json.loads((ROOT / "db/namespace.json").read_text())
schema = (ROOT / "db/schema.sql").read_text()
bootstrap = (ROOT / "db/bootstrap.sql").read_text()
grants = (ROOT / "db/grants.sql").read_text()
persistence = (ROOT / "src/persistence.rs").read_text()
readiness = (ROOT / "src/readiness.rs").read_text()
main = (ROOT / "src/main.rs").read_text()

if manifest["namespaceId"] != NAMESPACE:
    fail("namespace id drift")
if manifest["database"]["schema"] != NAMESPACE:
    fail("physical schema drift")
if manifest["database"]["minimumMajorVersion"] != 17:
    fail("minimum PostgreSQL version drift")
if manifest["roles"] != ROLES:
    fail("database role drift")
if manifest["declarativeMigration"]["revision"] != DPM_REVISION:
    fail("declarative migration revision drift")
if manifest.get("readiness") != {
    "endpoint": "/readyz",
    "runtimeRole": "canonical_cloud__quote__api_rw",
    "livenessEndpoint": "/healthz",
    "failClosed": True,
}:
    fail("readiness manifest drift")

schema_digest = hashlib.sha256(schema.encode()).hexdigest()
if schema_digest != manifest["declarativeMigration"]["sourceSha256"]:
    fail("schema digest drift")

for forbidden in (
    "IF NOT EXISTS",
    "BEGIN;",
    "COMMIT;",
    "GRANT ",
    "REVOKE ",
    "ALTER OWNER",
):
    if forbidden in schema:
        fail(f"declarative schema contains lifecycle statement: {forbidden}")

if schema.count(f"CREATE SCHEMA {NAMESPACE};") != 1:
    fail("declarative schema must create exactly one owned namespace")

table_names = (
    "canonical_context",
    "canonical_quote",
    "canonical_quote_event",
    "canonical_model_attempt",
)
for table in table_names:
    qualified = f"{NAMESPACE}.{table}"
    if f"CREATE TABLE {qualified}" not in schema:
        fail(f"missing qualified table: {qualified}")
    if f"ALTER TABLE {qualified}\n    FORCE ROW LEVEL SECURITY" not in schema:
        fail(f"forced RLS missing for {qualified}")
    if table not in persistence:
        fail(f"runtime persistence omits {table}")

required_constraints = (
    "canonical_context_json_object_check",
    "canonical_context_id_owner_unique",
    "canonical_quote_request_json_object_check",
    "canonical_quote_context_snapshot_json_object_check",
    "canonical_quote_analysis_json_object_check",
    "canonical_quote_id_owner_unique",
    "canonical_quote_context_owner_fk",
    "canonical_quote_event_details_json_object_check",
    "canonical_quote_event_quote_owner_fk",
    "canonical_model_attempt_status_finished_check",
    "canonical_model_attempt_time_order_check",
    "canonical_model_attempt_quote_owner_fk",
)
for constraint in required_constraints:
    if schema.count(f"CONSTRAINT {constraint}") != 1:
        fail(f"required constraint drift: {constraint}")

for owner_fk in (
    "FOREIGN KEY (context_record_id, owner_subject)",
    "FOREIGN KEY (quote_id, owner_subject)",
):
    if owner_fk not in schema:
        fail(f"owner-consistent foreign key missing: {owner_fk}")

for json_column in (
    "context_json",
    "request_json",
    "context_snapshot_json",
    "analysis_json",
    "details_json",
):
    if f"jsonb_typeof({json_column})" not in schema:
        fail(f"JSON object shape check missing: {json_column}")

if "public.canonical_" in persistence:
    fail("runtime SQL must not bind Canonical objects to public")
if "canonical_cloud__quote" not in bootstrap:
    fail("role-level Canonical search_path pin is missing")

for role in ROLES.values():
    if role not in bootstrap:
        fail(f"bootstrap omits role {role}")

for required in (
    "REVOKE CREATE ON SCHEMA public FROM PUBLIC;",
    "CREATE SCHEMA IF NOT EXISTS canonical_cloud__quote",
    "NOT rolcanlogin",
    "rolinherit",
    "rolbypassrls",
    "pg_has_role(",
):
    if required not in bootstrap:
        fail(f"bootstrap hardening omits {required}")

for required in (
    "canonical_cloud__quote__web_ro",
    "GRANT SELECT, INSERT, UPDATE",
    "canonical_quote_event API privilege contract is not append-only",
    "has_sequence_privilege",
    "has_function_privilege",
    "refusing grants because",
):
    if required not in grants:
        fail(f"grant contract omits {required}")
if "GRANT ALL" in grants:
    fail("broad grants are forbidden")

for required in (
    'route(\n        "/readyz"',
    "canonical_cloud__quote__api_rw",
    "canonical_quote_event_quote_owner_fk",
    "canonical_model_attempt_quote_owner_fk",
    "canonical_model_attempt_status_finished_check",
    "has_table_privilege",
    "rolbypassrls",
):
    if required not in readiness:
        fail(f"readiness contract omits {required}")
if "merge(readiness::router(readiness_database))" not in main:
    fail("binary does not serve the fail-closed readiness router")

workflow = (ROOT / ".github/workflows/declarative-postgres.yml").read_text()
for required in (
    "postgres:17",
    DPM_REVISION,
    "scripts/test-declarative-postgres.sh",
    "scripts/verify-declarative-db-contract.py",
    "persist-credentials: false",
):
    if required not in workflow:
        fail(f"database workflow omits {required}")

print(
    json.dumps(
        {
            "namespace": NAMESPACE,
            "schema_sha256": schema_digest,
            "dpm_revision": DPM_REVISION,
            "tables": list(table_names),
            "required_constraints": list(required_constraints),
            "runtime_readiness": "/readyz",
            "web_direct_database_access": False,
        },
        sort_keys=True,
    )
)
