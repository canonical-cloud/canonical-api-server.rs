#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
check_args=()

if [[ "${1:-}" == "--check" ]]; then
  check_args=(--check)
  shift
fi

if (($#)); then
  printf 'usage: %s [--check]\n' "$0" >&2
  exit 64
fi

run_docs() {
  if [[ -n "${ORES_STACK_ROOT:-}" ]]; then
    "$ORES_STACK_ROOT/scripts/api-docs.sh" "$@"
  else
    "${ORES_STACK_BIN:-ores-stack}" docs "$@"
  fi
}

run_docs --root "$repo_root" --format markdown --out generated/api-docs.md "${check_args[@]}"
run_docs --root "$repo_root" --format html --out generated/api-docs.html "${check_args[@]}"
