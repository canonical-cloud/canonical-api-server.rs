# canonical-api-server.rs

Rust REST and WebSocket backend for Canonical customer applications. The first
service surface is a durable, authenticated compliance-quote workflow using
Axum, SeaORM/PostgreSQL, Supabase identity, and server-side Gemini analysis.

## Boundaries

- `canonical-web-server.rs` owns browser sessions, Maud/HTMX pages, and the
  `app.canonical.plus` origin.
- This service owns quote persistence, durable work claiming, Gemini calls, and
  quote-event WebSockets.
- Direct SDK/API callers authenticate with a Supabase bearer token.
- The web server authenticates internally with a separate random service token
  plus an already-verified user id. Deploy the services behind a NetworkPolicy
  and rotate this credential independently of Supabase and Gemini.
- Gemini access and refresh credentials never enter browser HTML or JavaScript.

## API

- `POST /v1/quotes` creates a private quote and returns `202 Accepted`.
- `GET /v1/quotes` lists the caller's newest 50 quotes.
- `GET /v1/quotes/{quote_id}` returns only a quote owned by the caller.
- `GET /v1/ws` upgrades an authenticated HTTP request and emits
  `quote.updated` events filtered to the caller.
- `GET /healthz` and `GET /readyz` expose liveness and database readiness.

Every quote is persisted before analysis. Workers atomically claim rows with
`FOR UPDATE SKIP LOCKED`, attach a lease, recover abandoned leases, retry with
bounded backoff, and store only stable error codes. PostgreSQL is therefore the
durable queue; WebSockets are hints and REST remains authoritative.

## Context and Gemini

`context/quote-analysis.md` is compiled into the binary as the stable policy.
Each job also loads the active version of `QUOTE_CONTEXT_KEY` from
`canonical_context`. The prompt labels customer JSON as untrusted data and the
model response must pass strict schema, size, range, and line-item-total checks
before it becomes a quote.

The service requires `GEMINI_API_KEY` and defaults `GEMINI_MODEL` to the
published `gemini-3.1-pro-preview` endpoint. Deployments may override the model
without rebuilding, but production configuration should pin an explicitly
approved Pro model and re-certify the structured-output contract whenever that
value changes. The API key is sent only in the server-side `x-goog-api-key`
header.

Seed a database context after applying the schema, for example:

```sql
INSERT INTO canonical_context
  (id, context_key, version, markdown, metadata, active)
VALUES
  ('00000000-0000-4000-8000-000000000001', 'quote_analysis', 1,
   '# Pricing context\n\nDescribe current rate cards, delivery constraints, and exclusions.',
   '{"owner":"sales-operations"}'::jsonb, true);
```

## Operations

Runtime and migration identities are intentionally separate:

```bash
MIGRATION_DATABASE_URL='postgresql://...' cargo run -- migrate
DATABASE_URL='postgresql://...' cargo run -- serve
```

The runtime process performs no DDL. See `.env.example` for the complete secret
and tuning contract. Do not commit real values.

## Repository dependencies

The Zed package manifest declares the intended layering:

- `canonical-interfaces` supplies transport contracts.
- `canonical-lib` supplies reusable Canonical domain behavior as it is extracted.
- this API server is consumed by `canonical-web-server.rs`, clients, CLI, and
  Flutter apps; infrastructure consumes its container artifact rather than its
  source tree.
