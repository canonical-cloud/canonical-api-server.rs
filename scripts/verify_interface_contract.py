#!/usr/bin/env python3
"""Verify the Canonical interface edge across Zed, Cargo, lock, and source."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def fail(message: str) -> None:
    print(f"interface-contract: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_toml(path: Path) -> dict:
    try:
        with path.open("rb") as handle:
            value = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {path.relative_to(ROOT)}: {error}")
    if not isinstance(value, dict):
        fail(f"{path.relative_to(ROOT)} must contain a TOML document")
    return value


def main() -> None:
    try:
        lock = json.loads((ROOT / "interfaces-source.lock.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse interfaces-source.lock.json: {error}")
    if not isinstance(lock, dict) or lock.get("schema_version") != 1:
        fail("interfaces-source.lock.json must use schema_version 1")

    coordinate = lock.get("coordinate")
    requirement = lock.get("version_requirement")
    interface_revision = lock.get("interface_revision")
    bridge = lock.get("cargo_bridge")
    if coordinate != "canonical-cloud/canonical-interfaces":
        fail("unexpected interface coordinate")
    if requirement != "^0.1.0":
        fail("unexpected interface version requirement")
    if not isinstance(interface_revision, str) or re.fullmatch(r"[0-9a-f]{40}", interface_revision) is None:
        fail("interface_revision must be a full lowercase Git SHA")
    if not isinstance(bridge, dict):
        fail("cargo_bridge must be an object")
    if bridge.get("package") != "canonical-lib":
        fail("cargo_bridge must identify canonical-lib")
    if bridge.get("git") != "https://github.com/canonical-cloud/canonical-lib-core":
        fail("unexpected canonical-lib Git source")
    bridge_revision = bridge.get("revision")
    if not isinstance(bridge_revision, str) or re.fullmatch(r"[0-9a-f]{40}", bridge_revision) is None:
        fail("cargo bridge revision must be a full lowercase Git SHA")

    zpkg = load_toml(ROOT / ".zpkg.toml")
    package = zpkg.get("package")
    if not isinstance(package, dict):
        fail(".zpkg.toml is missing [package]")
    if package.get("role") != "server" or package.get("family") != "canonical":
        fail(".zpkg.toml must identify the canonical server role")
    zed_dependencies = zpkg.get("dependencies")
    if not isinstance(zed_dependencies, dict) or zed_dependencies.get(coordinate) != requirement:
        fail("Zed interface dependency differs from the provenance lock")

    cargo = load_toml(ROOT / "Cargo.toml")
    cargo_dependencies = cargo.get("dependencies")
    if not isinstance(cargo_dependencies, dict):
        fail("Cargo.toml is missing [dependencies]")
    native_bridge = cargo_dependencies.get("canonical-lib")
    if not isinstance(native_bridge, dict):
        fail("Cargo.toml must depend on canonical-lib")
    if native_bridge.get("git") != bridge.get("git") or native_bridge.get("rev") != bridge_revision:
        fail("Cargo canonical-lib source differs from the provenance lock")

    cargo_lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    required_fragments = (
        'name = "canonical-interfaces"',
        interface_revision,
        'name = "canonical-lib"',
        bridge_revision,
    )
    for fragment in required_fragments:
        if fragment not in cargo_lock:
            fail(f"Cargo.lock is missing {fragment!r}")

    consumers = lock.get("consumers")
    if not isinstance(consumers, dict):
        fail("consumers must be an object")
    runtime = consumers.get("runtime")
    domain = consumers.get("domain_contract")
    if runtime != "src/main.rs" or domain != "src/contract.rs":
        fail("consumer paths differ from the reviewed contract")
    runtime_source = (ROOT / runtime).read_text(encoding="utf-8")
    domain_source = (ROOT / domain).read_text(encoding="utf-8")
    if "use canonical_lib::interfaces::QuoteRequest;" not in runtime_source:
        fail("runtime does not import the generated QuoteRequest type")
    if "std::any::type_name::<QuoteRequest>()" not in runtime_source:
        fail("runtime does not establish a compile-time interface edge")
    if "use canonical_lib::interfaces::{" not in domain_source:
        fail("domain contract does not consume the generated interface re-export")
    if "canonical_lib::validate_quote_request" not in domain_source:
        fail("domain contract no longer uses the canonical validator")

    print(
        "interface-contract: Zed direct edge, Cargo bridge, Cargo.lock, and "
        f"source consumers agree on {coordinate}@{interface_revision}"
    )


if __name__ == "__main__":
    main()
