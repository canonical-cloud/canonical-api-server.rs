#!/usr/bin/env bash
set -euo pipefail

repo_root="${GITHUB_WORKSPACE:-$(pwd)}"
canary_root="${repo_root}/tmp/zed-private-deps"
source_root="${canary_root}/workspace/canonical-lib-core"
expected_sha="d2f7371f01f257fbaee532b923f4c4b0d2c4dff4"

test -x "$(command -v zed)"
test -d "${source_root}/.git"
actual_sha="$(git -C "${source_root}" rev-parse HEAD)"
test "${actual_sha}" = "${expected_sha}"

grep -q '^name = "canonical-lib"$' "${source_root}/Cargo.toml"
grep -q '^version = "0.1.0"$' "${source_root}/Cargo.toml"
grep -q '^name = "canonical-lib-core"$' "${source_root}/.zpkg.toml"
grep -q '^version = "0.1.0"$' "${source_root}/.zpkg.toml"

cat > "${canary_root}/.zpkg.toml" <<'EOF'
[package]
org = "canonical-cloud"
name = "canonical-api-zed-private-deps"
version = "0.0.0"
description = "CI-only exact-source Zed dependency projection"
license = "MIT"
language = "rust"

[package.repository]
vcs = "git"
url = "https://github.com/canonical-cloud/canonical-api-server.rs"

[dependencies]
"canonical-cloud/canonical-lib-core" = "=0.1.0"

[workspace]
members = ["workspace/*"]

[install]
adapter = "rust"
dir = "zed_modules"
EOF

(
  cd "${canary_root}"
  zed install

  test -f .zed/cargo-paths.toml
  grep -q 'canonical-lib' .zed/cargo-paths.toml
  grep -q 'workspace/canonical-lib-core' .zed/cargo-paths.toml

  if grep -Eiq 'x-access-token|CANONICAL_LIB_READ_TOKEN|github\.com/.+@' .zed/cargo-paths.toml; then
    echo "Zed Cargo adapter leaked credential-bearing source data" >&2
    exit 1
  fi
)

echo "zed private Rust dependency projection passed for ${expected_sha}"
