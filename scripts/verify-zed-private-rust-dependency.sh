#!/usr/bin/env bash
set -euo pipefail

repo_root="${GITHUB_WORKSPACE:-$(pwd)}"
canary_root="${repo_root}/tmp/zed-private-deps"
source_root="${canary_root}/workspace/canonical-lib-core"
expected_sha="d2f7371f01f257fbaee532b923f4c4b0d2c4dff4"
patch_mode="${ZED_GIT_SOURCE_PATCH_MODE:-legacy}"

test -x "$(command -v zed)"
test -d "${source_root}/.git"
actual_sha="$(git -C "${source_root}" rev-parse HEAD)"
test "${actual_sha}" = "${expected_sha}"

grep -q '^name = "canonical-lib"$' "${source_root}/Cargo.toml"
grep -q '^version = "0.1.0"$' "${source_root}/Cargo.toml"
grep -q '^name = "canonical-lib-core"$' "${source_root}/.zpkg.toml"
grep -q '^version = "0.1.0"$' "${source_root}/.zpkg.toml"

cat >"${canary_root}/Cargo.toml" <<EOF
[package]
name = "canonical-api-zed-private-deps-canary"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
canonical-lib = { version = "=0.1.0", git = "https://github.com/canonical-cloud/canonical-lib-core", rev = "${expected_sha}" }
EOF
mkdir -p "${canary_root}/src"
printf '%s\n' 'pub fn canary() {}' >"${canary_root}/src/lib.rs"

cat >"${canary_root}/.zpkg.toml" <<'EOF'
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
  test -L zed_modules/canonical-cloud/canonical-lib-core
  test "$(readlink -f zed_modules/canonical-cloud/canonical-lib-core)" = "$(readlink -f workspace/canonical-lib-core)"
  grep -Fq '"canonical-lib"' .zed/cargo-paths.toml
  grep -Fq 'zed_modules/canonical-cloud/canonical-lib-core' .zed/cargo-paths.toml

  if grep -Eiq 'x-access-token|CANONICAL_LIB_READ_TOKEN|github\.com/.+@' .zed/cargo-paths.toml; then
    echo "Zed Cargo adapter leaked credential-bearing source data" >&2
    exit 1
  fi

  case "$patch_mode" in
    legacy)
      if grep -Fq '[patch."https://github.com/canonical-cloud/canonical-lib-core"]' .zed/cargo-paths.toml; then
        echo "::warning::released Zed unexpectedly contains post-#446 Git-source patch behavior"
      fi
      ;;
    required)
      grep -Fq '[patch."https://github.com/canonical-cloud/canonical-lib-core"]' .zed/cargo-paths.toml
      grep -Fq '"canonical-lib" = { path = "zed_modules/canonical-cloud/canonical-lib-core" }' .zed/cargo-paths.toml

      if awk '
        /^\[patch\.crates-io\]$/ { in_crates_io = 1; next }
        /^\[/ { in_crates_io = 0 }
        in_crates_io && /"canonical-lib"/ { found = 1 }
        END { exit found ? 0 : 1 }
      ' .zed/cargo-paths.toml; then
        echo "Zed incorrectly patched canonical-lib through crates.io instead of its declared Git source" >&2
        exit 1
      fi
      ;;
    *)
      echo "invalid ZED_GIT_SOURCE_PATCH_MODE: $patch_mode" >&2
      exit 2
      ;;
  esac
)

echo "zed private Rust dependency projection passed for ${expected_sha} (mode=${patch_mode})"
