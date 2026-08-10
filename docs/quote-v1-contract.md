# Canonical quote API v1 implementation

This branch implements the current public quote-v1 contract from `canonical-cloud/canonical-interfaces`.

## Public API

- `POST /api/v1/quotes` requires `Idempotency-Key` and returns the canonical accepted response.
- `GET /api/v1/quotes/{quoteId}` returns `QuoteDetail` without persistence, credential, tenant, or model-attempt internals.
- `GET /api/v1/quotes` returns a bounded, owner-scoped `QuoteListResponse`.
- `POST /api/v1/quotes/{quoteId}/retry` requires `Idempotency-Key` and transitions only failed quotes.
- `GET /api/v1/quotes/{quoteId}/events` upgrades to the authenticated persisted event stream.

The legacy `/v1/quotes` route family remains a compatibility alias. `/api/v1/quotes` is authoritative.

## Authority boundaries

The trusted web tier sends only the isolated internal service credential and the already verified Shared Auth subject. Browser payloads cannot select owner identity, tenant identity, application Markdown, or a database context record. PostgreSQL forced RLS and owner-consistent foreign keys remain authoritative.

## PostgreSQL additions

`canonical_quote_operation` stores owner-scoped create/retry idempotency decisions. It is forced-RLS, append-only for the runtime role, constrained to create/retry operations, and linked to quotes with a composite quote-owner foreign key.

The branch must pass repository CI and a fresh external `declarative-migrations-test` PostgreSQL 17/18, drift-repair, failure-atomicity, and backup/restore matrix before promotion.
