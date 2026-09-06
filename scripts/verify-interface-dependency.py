#!/usr/bin/env python3
"""Fail closed when Canonical API interface dependency authorities diverge."""
from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def fail(message: str) -> None:
    print(f"interface-contract: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            value = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        fail(f"cannot parse {path.relative_to(ROOT)}: {exc}")
    if not isinstance(value, dict):
        fail(f"{path.relative_to(ROOT)} must contain a TOML table")
    return value


def main() -> None:
    lock = json.loads((ROOT / "contracts/interfaces-source.lock.json").read_text(encoding="utf-8"))
    if not isinstance(lock, dict):
        fail("interfaces-source lock must contain a JSON object")
    zpkg = load_toml(ROOT / ".zpkg.toml")
    cargo = load_toml(ROOT / "Cargo.toml")

    coordinate = lock.get("coordinate")
    version = lock.get("version_requirement")
    repository = lock.get("repository")
    crate = lock.get("rust_crate")
    revision = lock.get("rust_revision")
    if not all(isinstance(value, str) and value for value in (coordinate, version, repository, crate, revision)):
        fail("interfaces-source lock has incomplete required fields")
    if re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        fail("interface revision must be a full lowercase Git SHA")

    if zpkg.get("dependencies", {}).get(coordinate) != version:
        fail("Zed does not declare the locked canonical interface package")
    cargo_dep = cargo.get("dependencies", {}).get(crate)
    if not isinstance(cargo_dep, dict):
        fail("Cargo must directly depend on canonical-interfaces")
    if cargo_dep.get("version") != "=0.1.0" or cargo_dep.get("git") != repository or cargo_dep.get("rev") != revision:
        fail("Cargo canonical-interfaces source differs from the reviewed lock")

    domain = lock.get("domain_library")
    if not isinstance(domain, dict):
        fail("domain_library lock is missing")
    domain_dep = cargo.get("dependencies", {}).get(domain.get("crate"))
    if not isinstance(domain_dep, dict):
        fail("Cargo canonical domain library dependency is missing")
    if domain_dep.get("git") != domain.get("repository") or domain_dep.get("rev") != domain.get("revision"):
        fail("canonical-lib source differs from the reviewed lock")

    module = (ROOT / "src/interface_contract.rs").read_text(encoding="utf-8")
    main = (ROOT / "src/main.rs").read_text(encoding="utf-8")
    for marker in (
        "use canonical_interfaces::{HealthStatus, HealthStatusStatus};",
        "Json<HealthStatus>",
        "HealthStatusStatus::Ok",
    ):
        if marker not in module:
            fail(f"generated health contract consumer is missing {marker!r}")
    if "mod interface_contract;" not in main or ".merge(interface_contract::router())" not in main:
        fail("the generated interface router is not mounted by the API server")

    lifecycle = zpkg.get("lifecycle", {})
    for phase in ("pre-build", "pre-publish"):
        config = lifecycle.get(phase)
        expected = f"sh ./.zpkg/{phase}"
        if not isinstance(config, dict) or config.get("mode") not in {"replace", "override"}:
            fail(f"lifecycle.{phase} must explicitly replace convention discovery")
        if config.get("command") != expected:
            fail(f"lifecycle.{phase} must execute {expected}")
        hook = ROOT / ".zpkg" / phase
        if hook.is_symlink() or not hook.is_file():
            fail(f"missing regular lifecycle hook: {hook.relative_to(ROOT)}")

    print(
        "interface-contract: Zed, direct Cargo, domain-library, route mounting, "
        "and generated health types are aligned"
    )


if __name__ == "__main__":
    main()
