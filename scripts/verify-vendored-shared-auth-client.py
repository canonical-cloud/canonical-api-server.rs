#!/usr/bin/env python3
"""Verify the reviewed, credentialless Shared Auth Rust client snapshot."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "vendor" / "shared-auth-clients"
PROVENANCE = VENDOR / "PROVENANCE.json"
EXPECTED_SOURCE = "https://github.com/shared-auth/shared-auth-clients"
EXPECTED_COMMIT = "a63d2817c92d0b018899e252536371b07d5622ea"
EXPECTED_SUBTREE = "clients/rust"


def fail(message: str) -> None:
    raise SystemExit(f"shared-auth vendor verification failed: {message}")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    try:
        provenance = json.loads(PROVENANCE.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"invalid provenance record: {error}")

    if provenance.get("source") != EXPECTED_SOURCE:
        fail("unexpected upstream repository")
    if provenance.get("commit") != EXPECTED_COMMIT:
        fail("unexpected upstream commit")
    if provenance.get("subtree") != EXPECTED_SUBTREE:
        fail("unexpected upstream subtree")

    files = provenance.get("files")
    if not isinstance(files, dict) or not files:
        fail("file digest map is missing")

    expected_paths = {PROVENANCE.relative_to(VENDOR).as_posix(), *files.keys()}
    actual_paths = {
        path.relative_to(VENDOR).as_posix()
        for path in VENDOR.rglob("*")
        if path.is_file()
    }
    if actual_paths != expected_paths:
        fail(
            "file allowlist drifted: "
            f"missing={sorted(expected_paths - actual_paths)} "
            f"unexpected={sorted(actual_paths - expected_paths)}"
        )

    for relative, expected_digest in sorted(files.items()):
        path = VENDOR / relative
        if path.is_symlink() or not path.is_file():
            fail(f"{relative} is not a regular vendored file")
        observed = digest(path)
        if observed != expected_digest:
            fail(f"SHA-256 mismatch for {relative}: {observed}")

    manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    path_dependency = (
        'shared-auth-client = { path = '
        '"vendor/shared-auth-clients/clients/rust" }'
    )
    if path_dependency not in manifest:
        fail("Cargo.toml does not use the verified path dependency")

    lockfile = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    if "https://github.com/shared-auth/shared-auth-clients" in lockfile:
        fail("Cargo.lock still requires private Git access")
    if 'name = "shared-auth-client"' not in lockfile:
        fail("Cargo.lock does not include shared-auth-client")

    print(
        "verified Shared Auth Rust client snapshot "
        f"{EXPECTED_COMMIT} ({len(files)} source files)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
