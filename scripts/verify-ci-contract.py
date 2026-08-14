#!/usr/bin/env python3
"""Validate immutable quote, interface, PostgreSQL, and Gemini contracts."""

from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text()


def tracked(path: str) -> None:
    observed = subprocess.check_output(
        ["git", "ls-files", "--error-unmatch", path],
        cwd=ROOT,
        text=True,
    ).strip()
    assert observed == path, (path, observed)


def git_blob_sha(data: bytes) -> str:
    header = b"blob " + str(len(data)).encode() + b"\0"
    return hashlib.sha1(header + data).hexdigest()


def main() -> None:
    schema = read("db/schema.sql")
    bootstrap = read("db/bootstrap.sql")
    grants = read("db/grants.sql")
    persistence = read("src/persistence.rs")
    library = read("src/lib.rs")
    gemini = read("src/gemini.rs")
    cargo_lock = read("Cargo.lock")
    env_example = read(".env.example")
    declarative_workflow = read(".github/workflows/declarative-postgres.yml")

    cargo_toml = tomllib.loads(read("Cargo.toml"))
    zpkg_toml = tomllib.loads(read(".zpkg.toml"))
    interface_lock = json.loads(read("interfaces.lock.json"))
    namespace = json.loads(read("db/namespace.json"))

    dpm_revision = "d05a7880987ddaa271fa88b52c787390ef12b899"
    canonical_lib_revision = "8986ab23a18ca0ef14422f6348b264acbb9acc43"
    canonical_interfaces_revision = "0cab33c2b2a494d2368ef1da0ebe5d614b3a96ef"

    assert namespace["namespaceId"] == "canonical_cloud__quote"
    database = namespace["database"]
    assert database["engine"] == "postgresql"
    assert database["minimumMajorVersion"] == 17
    assert database["schema"] == "canonical_cloud__quote"
    assert database["placement"] == "canonical-only"
    assert database["publicObjectPolicy"] == "forbidden"
    assert namespace["declarativeMigration"]["revision"] == dpm_revision
    assert namespace["declarativeMigration"]["source"] == "db/schema.sql"
    assert namespace["declarativeMigration"]["sourceSha256"] == hashlib.sha256(
        schema.encode()
    ).hexdigest()
    assert namespace["roles"] == {
        "migrator": "canonical_cloud__quote__migrator",
        "apiRuntime": "canonical_cloud__quote__api_rw",
        "webRuntime": "canonical_cloud__quote__web_ro",
    }
    assert namespace["readiness"] == {
        "endpoint": "/readyz",
        "runtimeRole": "canonical_cloud__quote__api_rw",
        "livenessEndpoint": "/healthz",
        "failClosed": True,
    }

    assert interface_lock["schema_version"] == 1
    assert interface_lock["canonical_lib"] == {
        "repository": "canonical-cloud/canonical-lib",
        "commit": canonical_lib_revision,
    }
    assert interface_lock["canonical_interfaces"]["repository"] == (
        "canonical-cloud/canonical-interfaces"
    )
    assert interface_lock["canonical_interfaces"]["commit"] == (
        canonical_interfaces_revision
    )

    fixture_contract = interface_lock["canonical_interfaces"]["quote_request_fixture"]
    assert fixture_contract["path"] == "fixtures/quote/v1/request.json"
    fixture_path = ROOT / fixture_contract["path"]
    fixture_bytes = fixture_path.read_bytes()
    assert git_blob_sha(fixture_bytes) == fixture_contract["git_blob"]
    fixture = json.loads(fixture_bytes)
    assert fixture["answersVersion"] == 1
    assert isinstance(fixture["organizationName"], str) and fixture["organizationName"]
    assert isinstance(fixture["contactEmail"], str) and "@" in fixture["contactEmail"]
    assert set(fixture["frameworks"]) == {
        "soc2_type_2",
        "nist_csf_2",
        "nist_800_53",
        "hipaa",
    }

    canonical_lib_dependency = cargo_toml["dependencies"]["canonical-lib"]
    assert canonical_lib_dependency == {
        "version": "=0.1.0",
        "git": "https://github.com/canonical-cloud/canonical-lib",
        "rev": canonical_lib_revision,
    }
    assert "canonical-lib" in cargo_lock
    assert (
        f'source = "git+https://github.com/canonical-cloud/canonical-lib?rev='
        f'{canonical_lib_revision}#{canonical_lib_revision}"'
    ) in cargo_lock

    assert zpkg_toml["package"]["name"] == "canonical-api-server"
    assert zpkg_toml["package"]["repository"]["url"] == (
        "https://github.com/canonical-cloud/canonical-api-server.rs"
    )
    assert zpkg_toml["dependencies"]["canonical-cloud/canonical-lib"] == "^0.1.0"
    assert (
        zpkg_toml["dependencies"]["canonical-cloud/canonical-interfaces"] == "^0.1.0"
    )

    assert "repository: declarative-migrations/declarative-postgres-migrate.rs" in (
        declarative_workflow
    )
    assert f"ref: {dpm_revision}" in declarative_workflow

    assert 'include_str!("../fixtures/quote/v1/request.json")' in library
    assert 'DEFAULT_GEMINI_MODEL: &str = "gemini-3.6-flash"' in library
    assert "gemini-3.6-pro" not in library
    assert "GEMINI_MODEL=gemini-3.6-flash" in env_example
    assert "gemini-3.6-pro" not in env_example
    assert '"thinkingLevel": "high"' in gemini
    assert '"responseFormat"' in gemini
    assert "analysis_schema()" in gemini
    assert "thinkingBudget" not in gemini
    assert "responseMimeType" not in gemini
    assert "responseJsonSchema" not in gemini

    # Local development may use the in-memory store, while deployment readiness
    # remains fail-closed through the namespace and /readyz contracts above.
    assert 'database_url: env::var("DATABASE_URL").ok()' in library
    assert '"memory-only".into()' in library
    assert "set_config('app.current_subject', $1, true)" in persistence

    for forbidden in ("IF NOT EXISTS", "BEGIN;", "COMMIT;", "GRANT ", "REVOKE "):
        assert forbidden not in schema, forbidden
    assert schema.count("CREATE SCHEMA canonical_cloud__quote;") == 1

    table_names = (
        "canonical_context",
        "canonical_quote",
        "canonical_quote_event",
        "canonical_model_attempt",
    )
    for table in table_names:
        assert f"CREATE TABLE canonical_cloud__quote.{table}" in schema
        assert (
            f"ALTER TABLE canonical_cloud__quote.{table}\n"
            "    FORCE ROW LEVEL SECURITY"
        ) in schema
        assert table in persistence

    for constraint in (
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
    ):
        assert schema.count(f"CONSTRAINT {constraint}") == 1, constraint

    for policy in (
        "canonical_context_owner_policy",
        "canonical_quote_owner_policy",
        "canonical_quote_event_owner_policy",
        "canonical_model_attempt_owner_policy",
    ):
        assert schema.count(f"CREATE POLICY {policy}") == 1, policy

    for role in (
        "canonical_cloud__quote__migrator",
        "canonical_cloud__quote__api_rw",
        "canonical_cloud__quote__web_ro",
    ):
        assert role in bootstrap
    assert "NOT rolcanlogin" in bootstrap
    assert "rolinherit" in bootstrap
    assert "pg_has_role(" in bootstrap
    assert "canonical_quote_event API privilege contract is not append-only" in grants
    assert "has_sequence_privilege" in grants
    assert "has_function_privilege" in grants
    assert "GRANT ALL" not in grants

    for path in (
        "interfaces.lock.json",
        "fixtures/quote/v1/request.json",
        "db/namespace.json",
        ".github/workflows/declarative-postgres.yml",
        "scripts/verify-ci-contract.py",
    ):
        tracked(path)

    print("quote, interface, PostgreSQL, and Gemini contracts passed")


if __name__ == "__main__":
    main()
