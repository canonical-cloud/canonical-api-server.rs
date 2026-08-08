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

if manifest["namespaceId"] != NAMESPACE:
    fail("namespace id drift")
if manifest["database"]["schema"] != NAMESPACE:
    fail("physical schema drift")
if manifest["roles"] != ROLES:
    fail("database role drift")
if manifest["declarativeMigration"]["revision"] != DPM_REVISION:
    fail("declarative migration revision drift")

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

if "public.canonical_" in persistence:
    fail("runtime SQL must not bind Canonical objects to public")
if "canonical_cloud__quote" not in bootstrap:
    fail("role-level Canonical search_path pin is missing")

for role in ROLES.values():
    if role not in bootstrap:
        fail(f"bootstrap omits role {role}")

if "REVOKE CREATE ON SCHEMA public FROM PUBLIC;" not in bootstrap:
    fail("public CREATE revocation is missing")
if "CREATE SCHEMA IF NOT EXISTS canonical_cloud__quote" not in bootstrap:
    fail("bootstrap namespace creation is missing")
if "canonical_cloud__quote__web_ro" not in grants:
    fail("web role deny contract is missing")
if "GRANT SELECT, INSERT, UPDATE" not in grants:
    fail("API DML allowlist is missing")
if "GRANT ALL" in grants:
    fail("broad grants are forbidden")

workflow = (ROOT / ".github/workflows/declarative-postgres.yml").read_text()
for required in (
    "postgres:17",
    DPM_REVISION,
    "scripts/test-declarative-postgres.sh",
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
            "web_direct_database_access": False,
        },
        sort_keys=True,
    )
)
