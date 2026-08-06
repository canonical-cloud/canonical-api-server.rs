# Persistence contract

The API database identity must own only Canonical quote tables and must not read
shared-auth or unrelated tenant tables directly.

Required records:

- quote request: immutable owner/tenant, requested frameworks, target date, and
  input digests;
- context snapshot: reviewed Markdown digest plus the exact owner-scoped
  `canonical_context` record version used for analysis;
- model attempt: provider/model, bounded timing/error metadata, and no raw
  prompt in logs;
- status event: append-only sequence used to recover REST and WebSocket state.

Every lookup and mutation includes owner/tenant predicates in the same database
transaction. WebSocket notifications are disposable hints; durable status is
always recovered from PostgreSQL.
