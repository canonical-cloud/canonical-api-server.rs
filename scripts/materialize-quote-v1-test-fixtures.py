#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "src/gemini.rs"


def replace_once(old: str, new: str) -> None:
    text = TARGET.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"src/gemini.rs: expected one exact test-fixture match, found {count}"
        )
    TARGET.write_text(text.replace(old, new), encoding="utf-8")


def main() -> None:
    replace_once(
        "    use crate::{CanonicalContext, CreateQuoteRequest, OrganizationInput};\n",
        "    use crate::{contract::parse_quote_request, CanonicalContext};\n",
    )
    replace_once(
        "        let request = CreateQuoteRequest {\n"
        "            legacy_context_record_id: None,\n"
        "            frameworks: vec![\"soc2\".into(), \"hipaa\".into()],\n"
        "            markdown_context: \"# Product\\nHosted service\".into(),\n"
        "            notes: Some(\"Initial estimate\".into()),\n"
        "            organization: OrganizationInput {\n"
        "                employee_count: 42,\n"
        "                industry: \"Software\".into(),\n"
        "                legal_name: \"Example Incorporated\".into(),\n"
        "            },\n"
        "            target_date: Some(\"2027-01-15\".into()),\n"
        "        };\n",
        "        let mut request = parse_quote_request(\n"
        "            serde_json::from_str(include_str!(\n"
        "                \"../fixtures/quote/v1/request.json\"\n"
        "            ))\n"
        "            .unwrap(),\n"
        "        )\n"
        "        .unwrap();\n"
        "        request.application_markdown = \"# Product\\nHosted service\".into();\n",
    )
    print("materialized Canonical quote v1 Gemini test fixture")


if __name__ == "__main__":
    main()
