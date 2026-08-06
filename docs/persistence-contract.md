# Persistence contract

The API database identity owns only Canonical quote tables and never reads
Shared Auth, Supabase Auth, or unrelated tenant tables directly.

## Durable records

- `canonical_context`: owner-scoped operational context selected by the caller;
- `canonical_quote`: immutable normalized request, application Markdown,
  database-context snapshots, chosen model, status, structured analysis, and a
  bounded error code;
- `canonical_model_attempt`: provider/model and start/finish metadata only;
- `canonical_quote_event`: append-only status sequence used to recover the
  state represented by REST and WebSocket responses.

Raw API keys, internal service tokens, and provider error bodies are never
stored. Application logs contain quote IDs and bounded error codes, not prompts,
context, model output, or user identifiers.

## Ownership and transactions

Every lookup and mutation:

1. starts a PostgreSQL transaction;
2. installs the validated Shared Auth subject in
   `app.current_subject` and compatibility JWT settings;
3. applies an explicit owner predicate;
4. executes against tables with forced row-level security;
5. commits the quote mutation and durable event atomically.

The runtime login is a non-owner, non-superuser, non-`BYPASSRLS` role without
role memberships. WebSocket notifications are disposable hints; current state
is always recovered from `canonical_quote`.

## Context snapshot

Quote creation loads one active row using the composite predicate
`(context_record_id, owner_subject)`. The API stores copies of the selected
record's Markdown and JSON plus the application-controlled Markdown submitted
by the web tier. Later edits to `canonical_context` cannot silently alter the
inputs that produced an existing quote.

## Migration

[`../db/schema.sql`](../db/schema.sql) is idempotent for initial deployment and
documents runtime grants. Apply it through the reviewed migration identity.
Subsequent production changes should move to numbered, forward-only migrations
owned by `canonical-infra`.
