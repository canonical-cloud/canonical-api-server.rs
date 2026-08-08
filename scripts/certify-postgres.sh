#!/usr/bin/env bash
set -euo pipefail

DPM="${DPM_BIN:?DPM_BIN must point to the checksum-pinned dpm binary}"
TARGET="${MIGRATION_DATABASE_URL:?MIGRATION_DATABASE_URL is required}"
SHADOW="${SHADOW_DATABASE_URL:?SHADOW_DATABASE_URL is required}"
PSQL="${PSQL_BIN:-psql}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCHEMA="$ROOT/db/schema.sql"
GRANTS="$ROOT/db/runtime-grants.sql"

case "${APPLY_RUNTIME_GRANTS:-false}" in
  true|false) ;;
  *)
    echo "APPLY_RUNTIME_GRANTS must be true or false" >&2
    exit 64
    ;;
esac

"$DPM" apply \
  --source-sql "$SCHEMA" \
  --target "$TARGET" \
  --shadow "$SHADOW" \
  --yes

"$DPM" diff \
  --source-sql "$SCHEMA" \
  --target "$TARGET" \
  --shadow "$SHADOW" \
  --fail-on-diff \
  >/dev/null

"$DPM" verify \
  --source-sql "$SCHEMA" \
  --target "$TARGET" \
  --shadow "$SHADOW"

if [[ "${APPLY_RUNTIME_GRANTS:-false}" == "true" ]]; then
  "$PSQL" "$TARGET" -v ON_ERROR_STOP=1 -f "$GRANTS"
fi

printf 'Canonical quote PostgreSQL schema converged and verified.\n'
