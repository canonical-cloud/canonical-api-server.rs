#!/usr/bin/env bash
set -euo pipefail

DPM="${DPM_BIN:?DPM_BIN is required}"
POSTGRES_ADMIN_URL="${POSTGRES_ADMIN_URL:-postgres://postgres@127.0.0.1:5432/postgres}"
TEST_DATABASE="${CANONICAL_QUOTE_TEST_DATABASE:-canonical_quote_contract}"
TARGET_ADMIN_URL="postgres://postgres@127.0.0.1:5432/${TEST_DATABASE}"
MIGRATOR_URL="postgres://canonical_cloud__quote__migrator@127.0.0.1:5432/${TEST_DATABASE}"
API_URL="postgres://canonical_cloud__quote__api_rw@127.0.0.1:5432/${TEST_DATABASE}"
WEB_URL="postgres://canonical_cloud__quote__web_ro@127.0.0.1:5432/${TEST_DATABASE}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS="${ROOT}/artifacts/declarative-postgres"

mkdir -p "$ARTIFACTS"

cleanup() {
  psql "$POSTGRES_ADMIN_URL" -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${TEST_DATABASE} WITH (FORCE)" \
    >/dev/null 2>&1 || true

  for role in \
    canonical_cloud__quote__web_ro \
    canonical_cloud__quote__api_rw \
    canonical_cloud__quote__migrator
  do
    psql "$POSTGRES_ADMIN_URL" -v ON_ERROR_STOP=1 \
      -c "DROP ROLE IF EXISTS ${role}" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

cleanup
psql "$POSTGRES_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${TEST_DATABASE}" >/dev/null

psql "$TARGET_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -f "$ROOT/db/bootstrap.sql" >/dev/null

"$DPM" apply \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --yes \
  >"$ARTIFACTS/initial-apply.out"

psql "$TARGET_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -f "$ROOT/db/grants.sql" >/dev/null

"$DPM" diff \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --fail-on-diff \
  >"$ARTIFACTS/initial-diff.sql"

"$DPM" verify \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  >"$ARTIFACTS/initial-verify.out"

role_contract="$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT string_agg(
      rolname || ':' ||
      rolsuper::text || ':' ||
      rolcreatedb::text || ':' ||
      rolcreaterole::text || ':' ||
      rolreplication::text || ':' ||
      rolbypassrls::text,
      ',' ORDER BY rolname
    )
    FROM pg_roles
    WHERE rolname IN (
      'canonical_cloud__quote__api_rw',
      'canonical_cloud__quote__migrator',
      'canonical_cloud__quote__web_ro'
    )
  "
)"
expected_role_contract="$(
  printf '%s' \
    'canonical_cloud__quote__api_rw:false:false:false:false:false,' \
    'canonical_cloud__quote__migrator:false:false:false:false:false,' \
    'canonical_cloud__quote__web_ro:false:false:false:false:false'
)"
test "$role_contract" = "$expected_role_contract"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT pg_get_userbyid(nspowner)
    FROM pg_namespace
    WHERE nspname = 'canonical_cloud__quote'
  "
)" = "canonical_cloud__quote__migrator"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relkind IN ('r', 'p', 'S')
      AND pg_get_userbyid(relation.relowner)
          <> 'canonical_cloud__quote__migrator'
  "
)" = "0"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relname IN (
        'canonical_context',
        'canonical_quote',
        'canonical_quote_operation',
        'canonical_quote_event',
        'canonical_model_attempt'
      )
      AND relation.relrowsecurity
      AND relation.relforcerowsecurity
  "
)" = "5"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT has_schema_privilege(
      'canonical_cloud__quote__api_rw',
      'canonical_cloud__quote',
      'USAGE'
    )::int
  "
)" = "1"
test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT has_schema_privilege(
      'canonical_cloud__quote__api_rw',
      'canonical_cloud__quote',
      'CREATE'
    )::int
  "
)" = "0"
test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT has_schema_privilege(
      'canonical_cloud__quote__api_rw',
      'public',
      'CREATE'
    )::int
  "
)" = "0"
for table in canonical_quote canonical_quote_operation; do
  test "$(
    psql "$TARGET_ADMIN_URL" -Atqc "
      SELECT has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.${table}',
        'SELECT'
      )::int
    "
  )" = "0"
done

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT
      has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'SELECT'
      )::int || ':' ||
      has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'INSERT'
      )::int || ':' ||
      has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'UPDATE'
      )::int || ':' ||
      has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'DELETE'
      )::int
  "
)" = "1:1:0:0"

psql "$API_URL" -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
BEGIN;
SET LOCAL app.current_subject = 'owner-a';
INSERT INTO canonical_cloud__quote.canonical_context (
    id,
    owner_subject,
    name,
    context_markdown,
    context_json
)
VALUES (
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    'owner-a',
    'Owner A context',
    'Synthetic test context A',
    '{"source":"declarative-migrations-test"}'::jsonb
);
INSERT INTO canonical_cloud__quote.canonical_quote (
    id,
    owner_subject,
    context_record_id,
    request_json,
    application_context_markdown,
    context_snapshot_markdown,
    context_snapshot_json,
    gemini_model,
    status
)
VALUES (
    '11111111-1111-4111-8111-111111111111',
    'owner-a',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    '{"organizationName":"Owner A","frameworks":["soc2_type_2"]}'::jsonb,
    '# application',
    'Synthetic test context A',
    '{"source":"declarative-migrations-test"}'::jsonb,
    'gemini-3.6-flash',
    'queued'
);
INSERT INTO canonical_cloud__quote.canonical_quote_operation (
    owner_subject,
    idempotency_key,
    operation,
    quote_id,
    request_json
)
VALUES (
    'owner-a',
    'quote:test-owner-a',
    'create',
    '11111111-1111-4111-8111-111111111111',
    '{"organizationName":"Owner A","frameworks":["soc2_type_2"]}'::jsonb
);
COMMIT;

BEGIN;
SET LOCAL app.current_subject = 'owner-b';
INSERT INTO canonical_cloud__quote.canonical_context (
    id,
    owner_subject,
    name,
    context_markdown,
    context_json
)
VALUES (
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
    'owner-b',
    'Owner B context',
    'Synthetic test context B',
    '{"source":"declarative-migrations-test"}'::jsonb
);
COMMIT;
SQL

test "$(
  psql "$API_URL" -Atq <<'SQL'
BEGIN;
SET LOCAL app.current_subject = 'owner-a';
SELECT count(*)
FROM canonical_cloud__quote.canonical_context;
COMMIT;
SQL
)" = "1"

test "$(
  psql "$API_URL" -Atq <<'SQL'
BEGIN;
SET LOCAL app.current_subject = 'owner-b';
SELECT count(*)
FROM canonical_cloud__quote.canonical_quote_operation;
COMMIT;
SQL
)" = "0"

set +e
psql "$API_URL" -v ON_ERROR_STOP=1 >/dev/null 2>"$ARTIFACTS/forged-owner.err" <<'SQL'
BEGIN;
SET LOCAL app.current_subject = 'owner-a';
INSERT INTO canonical_cloud__quote.canonical_context (
    id,
    owner_subject,
    name
)
VALUES (
    'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
    'owner-b',
    'Forged owner'
);
COMMIT;
SQL
forged_owner_status=$?
set -e
test "$forged_owner_status" -ne 0
grep -Eqi 'row-level security|policy' "$ARTIFACTS/forged-owner.err"

set +e
psql "$API_URL" -v ON_ERROR_STOP=1 >/dev/null 2>"$ARTIFACTS/invalid-idempotency.err" <<'SQL'
BEGIN;
SET LOCAL app.current_subject = 'owner-a';
INSERT INTO canonical_cloud__quote.canonical_quote_operation (
    owner_subject,
    idempotency_key,
    operation,
    quote_id,
    request_json
)
VALUES (
    'owner-a',
    'bad key',
    'create',
    '11111111-1111-4111-8111-111111111111',
    '{}'::jsonb
);
COMMIT;
SQL
invalid_idempotency_status=$?
set -e
test "$invalid_idempotency_status" -ne 0
grep -Eqi 'check constraint|violates' "$ARTIFACTS/invalid-idempotency.err"

set +e
psql "$WEB_URL" -v ON_ERROR_STOP=1 \
  -c "SELECT count(*) FROM canonical_cloud__quote.canonical_quote" \
  >/dev/null 2>"$ARTIFACTS/web-read.err"
web_read_status=$?
set -e
test "$web_read_status" -ne 0
grep -Eqi 'permission denied|no permission' "$ARTIFACTS/web-read.err"

set +e
psql "$API_URL" -v ON_ERROR_STOP=1 \
  -c "UPDATE canonical_cloud__quote.canonical_quote_operation SET operation = 'retry'" \
  >/dev/null 2>"$ARTIFACTS/operation-update.err"
operation_update_status=$?
set -e
test "$operation_update_status" -ne 0
grep -Eqi 'permission denied|no permission' "$ARTIFACTS/operation-update.err"

set +e
psql "$API_URL" -v ON_ERROR_STOP=1 \
  -c "CREATE TABLE canonical_cloud__quote.api_must_not_create(id integer)" \
  >/dev/null 2>"$ARTIFACTS/api-ddl.err"
api_ddl_status=$?
set -e
test "$api_ddl_status" -ne 0
grep -Eqi 'permission denied|no permission' "$ARTIFACTS/api-ddl.err"

set +e
psql "$API_URL" -v ON_ERROR_STOP=1 \
  -c "CREATE TABLE public.api_must_not_create(id integer)" \
  >/dev/null 2>"$ARTIFACTS/public-ddl.err"
public_ddl_status=$?
set -e
test "$public_ddl_status" -ne 0
grep -Eqi 'permission denied|no permission' "$ARTIFACTS/public-ddl.err"

"$DPM" apply \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --yes \
  >"$ARTIFACTS/idempotent-apply.out"

psql "$TARGET_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -f "$ROOT/db/grants.sql" >/dev/null

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM canonical_cloud__quote.canonical_context
  "
)" = "2"
test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM canonical_cloud__quote.canonical_quote_operation
  "
)" = "1"

psql "$MIGRATOR_URL" -v ON_ERROR_STOP=1 \
  -c "ALTER TABLE canonical_cloud__quote.canonical_context ADD COLUMN rogue_note text" \
  >/dev/null

set +e
"$DPM" diff \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --fail-on-diff \
  >"$ARTIFACTS/drift.sql" 2>"$ARTIFACTS/drift.err"
drift_status=$?
set -e
test "$drift_status" -eq 2
grep -Eqi 'rogue_note|DROP COLUMN' "$ARTIFACTS/drift.sql" "$ARTIFACTS/drift.err"

set +e
"$DPM" apply \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --yes \
  >"$ARTIFACTS/destructive-gate.out" \
  2>"$ARTIFACTS/destructive-gate.err"
destructive_gate_status=$?
set -e
test "$destructive_gate_status" -eq 3
grep -q 'NOT CONVERGED' "$ARTIFACTS/destructive-gate.err"

"$DPM" apply \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --yes \
  --allow-destructive \
  >"$ARTIFACTS/remediation.out"

psql "$TARGET_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -f "$ROOT/db/grants.sql" >/dev/null

"$DPM" verify \
  --source-sql "$ROOT/db/schema.sql" \
  --target "$MIGRATOR_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  >"$ARTIFACTS/final-verify.out"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM information_schema.columns
    WHERE table_schema = 'canonical_cloud__quote'
      AND table_name = 'canonical_context'
      AND column_name = 'rogue_note'
  "
)" = "0"

test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM canonical_cloud__quote.canonical_context
  "
)" = "2"
test "$(
  psql "$TARGET_ADMIN_URL" -Atqc "
    SELECT count(*)
    FROM canonical_cloud__quote.canonical_quote_operation
    WHERE idempotency_key = 'quote:test-owner-a'
  "
)" = "1"

echo "Canonical quote PostgreSQL declarative certification passed"
