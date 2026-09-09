# Signed readiness-observation ingestion

Linear: `DEN-3938`

The API accepts versioned, signed observations from customer systems and other explicitly authorized read-only evidence connectors at:

- `POST /api/v1/readiness/observations`
- `POST /v1/readiness/observations`

The portable wire contract is owned by `canonical-cloud/canonical-interfaces` at `contracts/readiness-observation/v1/`. Its independently authored TypeSpec and JSON Schema authorities are admitted by `ORESoftware/typespec-json-schema-validator`; this server implements that accepted field and enum vocabulary and adds transport, tenant, replay, sequencing, reference-integrity, and persistence controls.

## Required request headers

- `Content-Type: application/json`
- `X-Canonical-Webhook-Id`: must equal the event body’s `eventId`
- `X-Canonical-Webhook-Timestamp`: Unix UTC seconds within the configured five-minute freshness window
- `X-Canonical-Webhook-Signature`: `v1=` plus the lowercase HMAC-SHA-256 of `<timestamp>.<exact body>`
- `X-Canonical-Webhook-Key-Id`: optional during a bounded key rotation; when absent, the server tries the active keys for the source
- `Content-Digest`: optional defense-in-depth `sha256:<lowercase-hex>` digest of the exact body

The endpoint requires HTTPS at ingress. The body is bounded to 256 KiB, rejects unknown JSON fields, and validates unique evidence identifiers, unique framework/control assertions, RFC 3339 timestamps, lowercase SHA-256 digests, approved evidence locators, and same-event evidence references.

## Source-bound keyring

`CANONICAL_READINESS_INGEST_KEYS_JSON` is an environment-only keyring. It is never accepted as a command-line value, logged, returned, or stored in Git. Example shape using placeholders only:

```json
{
  "keys": [
    {
      "keyId": "customer-ci-2026-09",
      "sourceId": "customer-ci",
      "organization": "org:customer",
      "subject": "subject:customer",
      "secret": "[AT-LEAST-32-BYTES-FROM-A-SECRET-STORE]",
      "notBefore": "2026-09-01T00:00:00Z",
      "notAfter": "2026-12-01T00:00:00Z"
    }
  ]
}
```

A key is bound to one source, organization, and owner subject. At most four active or staged keys may exist per source and 64 keys in one process. Invalid, duplicate, over-sized, weak, or time-inverted keyring entries fail startup.

## Idempotency and ordering

Within each owner/source stream, `sourceSequence` must increase monotonically. An exact replay of the same event ID, source sequence, and payload digest returns a duplicate receipt. Event-ID reuse with different content and sequence reuse with different content fail closed with `409`.

When PostgreSQL is configured, the runtime writes to the separate `canonical_cloud__readiness` namespace under row-level security. The runtime role receives only `SELECT` and `INSERT`; it receives no update, delete, truncate, reference, trigger, or schema-creation authority. The schema is delivered in `db/readiness-observation-schema.sql` with grants in `db/readiness-observation-grants.sql`. Deployments must apply both through the reviewed migration path before enabling customer keys.

Without PostgreSQL, the route uses a bounded memory-only store for local development and tests. That mode is not durable and is not suitable for sold monitoring or audit-evidence workflows.

## Evidence and assurance boundary

A successful receipt means the exact request body was authenticated, validated, ordered, and recorded. Every new receipt starts with `substantiveReview: "unreviewed"`. It does not mean that a control passed, evidence is complete or representative, an exception was approved, an auditor accepted the item, or any certification, authorization, attestation, legal conclusion, or assurance opinion exists.
