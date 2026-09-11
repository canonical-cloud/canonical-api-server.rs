# Readiness observation ingress

Linear: `DEN-3938`

This service can receive signed readiness observations from a customer-controlled system or an explicitly authorized read-only connector. It records what the source reported over time so Canonical Cloud can detect gaps and drift. Transport acceptance is not substantive evidence acceptance.

> An accepted receipt means the exact body was authenticated, admitted, and durably stored. It is not a passed control, audit opinion, attestation, certification, authorization, legal conclusion, or independent evaluator determination. Every new record starts with `substantiveReview: "unreviewed"`.

## Contract authority

The portable wire contract is owned by `canonical-cloud/canonical-interfaces` at immutable commit `bed2dacd7ccd8242ed4fba4d077b84a01a55e343`:

- `contracts/readiness-observation/v1/main.tsp`
- `contracts/readiness-observation/v1/authored.schema.json`
- `contracts/readiness-observation/v1/instances`

TypeSpec and JSON Schema are independent, human-maintained authorities. Neither overwrites or ranks above the other. This repository pins their admitted revision in `contracts/readiness-observation.lock.json` and re-runs `ORESoftware/typespec-json-schema-validator` at immutable commit `3171025cbe03a7026a71ce94eea18c910e1431b2` before accepting a Rust projection change.

## Routes

```text
POST /api/v1/readiness/sources/{source_id}/observations
POST /v1/readiness/sources/{source_id}/observations
```

`source_id` in the path must equal `sourceId` in the body. The request must use `Content-Type: application/json`, contain at most 262,144 bytes, and use the lowerCamelCase fields defined by the admitted contract. Unknown JSON fields fail closed.

## Authentication and exact-body signing

Required headers:

```text
X-Canonical-Webhook-Id: <eventId>
X-Canonical-Webhook-Timestamp: <Unix seconds>
X-Canonical-Webhook-Signature: v1=<lowercase HMAC-SHA256 hex>
```

The signature input is exactly:

```text
<timestamp>.<raw request body bytes>
```

The server compares signatures in constant time, rejects timestamps more than five minutes from its clock, does not trim or reserialize the signed body, and requires `X-Canonical-Webhook-Id` to equal the body’s `eventId`.

Source credentials are supplied only through `CANONICAL_READINESS_INGEST_KEYS_JSON` by the deployment secret store. A source entry binds a `sourceId`, organization, owner subject, key id, secret, and optional validity interval. Several entries may share a source id so a new key can overlap an old key during non-destructive rotation. Real secrets must never be committed, logged, placed in a URL, or copied into an issue or pull request.

## Ordering, idempotency, and chain integrity

For each authenticated owner and source:

1. The first `sourceSequence` is `1`.
2. Every later sequence is exactly the previous sequence plus one.
3. Re-delivery of the same event id, sequence, and payload digest returns the original receipt with `status: "duplicate"`.
4. Reusing an event id with different content returns `409 event-conflict`.
5. Reusing or skipping a source sequence returns `409 sequence-conflict`.

A PostgreSQL advisory transaction lock serializes one source stream. Each record stores the prior record digest and a new record digest that commits to the prior digest, payload digest, event id, and source sequence. This provides tamper-evident continuity without storing raw evidence objects.

## Evidence boundary

The request body may carry evidence metadata and digests, not credentials or unbounded evidence bodies. Each assertion may reference admitted evidence ids. The server rejects duplicate identifiers, missing references, malformed digests, future timestamps outside the bounded skew allowance, unsafe locators, credential-bearing URLs, query tokens, and fragments.

One evidence object may support assertions in several frameworks, but framework applicability, reported status, exceptions, approval, completion, and independent assurance remain distinct. A customer-reported `observed` value is still a report from that source; it does not become Canonical Cloud’s conclusion merely because the transport was valid.

## Persistence and readiness

Production admission fails closed without PostgreSQL. Tests may use the bounded in-memory store, but the production constructor never acknowledges an observation until the append-only ledger commits.

The ledger:

- is owner-scoped with forced row-level security;
- grants the API role only `SELECT` and `INSERT`;
- grants no direct table access to the web role;
- rejects update, delete, and truncate privileges;
- has unique event-id and source-sequence constraints;
- stores the exact payload digest, stable receipt id, authenticated key id, event metadata, chain digests, transport result, and initial substantive-review state.

`/readyz` returns ready only when the table, constraints, index, owner policy, ownership, runtime role, and exact privileges are present. The PostgreSQL 17 certification workflow also proves owner isolation, append-only denial, duplicate-sequence rejection, drift detection, destructive-change gating, and data preservation across declarative replay.

## Receipt

A successful new or duplicate delivery returns HTTP `202` with the admitted receipt shape:

```json
{
  "specVersion": "canonical.readiness.observation.receipt.v1",
  "receiptId": "rcpt_example",
  "eventId": "event.example.00000001",
  "sourceId": "source.example-ci",
  "sourceSequence": 1,
  "status": "accepted",
  "duplicate": false,
  "payloadSha256": "sha256:...",
  "receivedAt": "2026-09-09T05:00:00Z",
  "transportVerification": "signature-valid",
  "substantiveReview": "unreviewed"
}
```

Exact retries return the original `receiptId` and `receivedAt` so receivers can reconcile idempotently.

## Operational controls before production exposure

- Provision PostgreSQL through the declarative schema and grant workflows.
- Inject source keys from the approved secret store, with an owner and expiration for each key.
- Keep clocks synchronized and alert on stale-signature or sequence-conflict rates.
- Strip caller-supplied internal identity headers at Cloudflare and ingress.
- Apply a distributed per-source and per-IP rate limit before this service.
- Monitor `/readyz`; never route observation traffic while the database contract is incomplete.
- Define the separate reviewer-disposition workflow before representing any observation as accepted evidence.
