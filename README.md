# Canonical quote API

Rust REST and WebSocket boundary for `api.canonical.plus`.

`app.canonical.plus` remains owned by `canonical-web-server.rs`. Browser traffic is authenticated by the Canonical Shared Auth realm at the web origin and reaches this service over a private path with a dedicated internal token plus the already verified subject. Direct SDK and CLI traffic uses a short-lived Shared Auth bearer token that this API independently verifies through the exact Shared Auth verification endpoint.

## Contract authority

The public quote v1 wire contract is generated from `canonical-cloud/canonical-interfaces` at immutable revision `3415d6d97721b18ee6734659d73a95ff3d35a151`.
Axum and SeaORM service for authenticated Canonical quote analysis. It combines a versioned Markdown playbook with the active `canonical_context` PostgreSQL record, requests structured analysis from Gemini, persists the result, and broadcasts owner-scoped status events over WebSockets.

## Routes

| Route | Purpose |
| --- | --- |
| `GET /healthz` | Process health |
| `GET /readyz` | PostgreSQL readiness |
| `POST /v1/quotes` | Validate, analyze, and save a quote |
| `GET /v1/quotes` | List the signed-in user's quotes |
| `GET /v1/quotes/{id}` | Read one owned quote |
| `GET /ws/quotes` | Owner-scoped quote status events |

This repository owns HTTP/WebSocket transport, PostgreSQL persistence, and Gemini provider orchestration. It does not publish a second request, response, status, or framework model under quote v1. Internal persistence and provider metadata are mapped into the generated public types before a response leaves the service.

## Implemented quote flow

1. `POST /api/v1/quotes` accepts the generated lowerCamelCase `QuoteRequest`, validates its bounded framework/questionnaire fields, and derives owner identity only from the accepted credential.
2. The server selects Canonical context under its allow-listed policy. The shared context-key default is `quote-analysis`; a client-supplied context key is a bounded request, not database or tenant authority.
3. PostgreSQL stores the normalized generated request, selected context snapshot, application policy snapshot, status, timestamps, and append-only events before or during analysis.
4. Gemini receives bounded untrusted questionnaire/context input through a fixed Google origin with no redirects, bounded retries/responses, schema-constrained JSON, and a circuit breaker.
5. Public status progresses through `queued`, `analyzing`, and then `ready` or `failed`.
6. REST is authoritative. WebSocket events are owner-scoped delivery hints; clients recover durable state through REST after disconnects or process restarts.
7. A failed quote may be requeued through the owner-scoped retry endpoint.

The default model is `gemini-3.1-pro-preview` and remains configurable through `GEMINI_MODEL`. Prompts, context, raw model output, internal tokens, bearer tokens, and API keys are never written to application logs or returned in public payloads.

The generated analysis is preliminary scoping assistance for human review. It is not a certification, audit opinion, legal opinion, attestation, or guarantee.

## Public routes

| Method | Route | Purpose |
| --- | --- | --- |
| `GET` | `/healthz` | Liveness plus non-secret DB/Gemini configuration state |
| `POST` | `/api/v1/quotes` | Validate, persist, and asynchronously analyze a quote |
| `GET` | `/api/v1/quotes` | Owner-scoped paginated quote summaries |
| `GET` | `/api/v1/quotes/{quoteId}` | Owner-scoped durable quote detail/result |
| `POST` | `/api/v1/quotes/{quoteId}/retry` | Requeue a failed owner-scoped quote |
| WebSocket | `/api/v1/quotes/{quoteId}/events` | Owner-scoped persisted status events |

## Authentication matrix

### Direct `api.canonical.plus`

Direct clients send a short-lived Shared Auth bearer token in the `Authorization` header. The API verifies it by calling the exact `SHARED_AUTH_VERIFY_URL`, refuses redirects, bounds timeouts, and accepts the verified subject only from the Shared Auth response. Browser cookies and query-string bearer tokens are not accepted on the API host.

### Private web-to-API path

The trusted Canonical web tier sends exactly:

- `x-canonical-internal-token`: a dedicated random web/API service credential of at least 32 bytes, compared in constant time;
- `x-canonical-subject`: the verified Shared Auth subject.

The API does not accept `x-canonical-user-id`, `x-canonical-user-email`, `x-canonical-service-token`, or request-body owner/tenant fields as identity authority. Cloudflare, ingress, and the web tier must remove caller-supplied internal and identity headers before forwarding.

## Public wire boundary

Public JSON uses lowerCamelCase and generated types including:

- `QuoteRequest`;
- `QuoteSubmissionResponse`;
- `QuoteDetail` and `QuoteSummary`/`QuoteListResponse`;
- `QuoteRetryResponse`;
- `QuoteStatusEvent`;
- `QuoteEstimate` and `QuoteProblem`.

Public identifiers use `quoteId`; statuses are exactly `queued`, `analyzing`, `ready`, and `failed`. Model-attempt rows, persistence labels, raw prompts/responses, internal context identifiers, service credentials, and tenant internals are not exposed.

## PostgreSQL

Apply [`db/schema.sql`](db/schema.sql) through the migration identity, not the runtime service login. The schema provides:

- `canonical_context` with owner-scoped context data;
- `canonical_quote` with normalized request/context snapshots and structured result;
- append-only `canonical_quote_event`;
- provider metadata in `canonical_model_attempt`;
- forced row-level security based on transaction-local `app.current_subject`.

The runtime login must be non-owner, non-superuser, non-`BYPASSRLS`, and receive only the documented grants. Every operation sets the verified subject inside the same transaction and also includes an explicit owner predicate.

`DATABASE_URL` is optional only for local routing tests. Without it, quote state is labelled memory-only and restart recovery is unavailable. Production must configure PostgreSQL.

## Configuration

Required in production:

- `CANONICAL_INTERNAL_AUTH_TOKEN` — private web/API service credential;
- `SHARED_AUTH_VERIFY_URL` — exact `/shared-auth/auth/verify` endpoint for direct bearer verification;
- `DATABASE_URL` — owner-scoped PostgreSQL runtime connection;
- `GEMINI_API_KEY` — server-side Gemini credential.

`GEMINI_MODEL` defaults to `gemini-3.1-pro-preview`. `BIND_ADDRESS` defaults to `0.0.0.0:8080`.

When the Gemini key is absent, health remains available and accepted development requests transition to `failed` with a bounded public problem. No provider error body is returned or logged.
The API is not directly internet-trusted. The Canonical Cloudflare Worker removes incoming identity headers, verifies the user's shared-auth JWT, and adds a short-lived HMAC assertion. The API verifies that assertion over the exact method and path before serving protected routes.

## Configuration

Required environment variables:

- `DATABASE_URL`
- `GEMINI_API_KEY`
- `ORIGIN_ASSERTION_SECRET` (at least 32 bytes and identical to the Worker secret)

Optional variables:

- `BIND_ADDR` (default `0.0.0.0:8081`)
- `GEMINI_MODEL` (default `gemini-3.1-pro-preview`)
- `QUOTE_CONTEXT_MARKDOWN_PATH` (default `context/quote-analysis.md`)
- `ORIGIN_ASSERTION_MAX_AGE_SECONDS` (default `60`)

Apply [`db/schema.sql`](db/schema.sql) to the Canonical Supabase/PostgreSQL database before deploying. Kubernetes resources are in [`deploy/k8s/all.yaml`](deploy/k8s/all.yaml); replace the image digest and provide the referenced ExternalSecret values through the cluster secret store.

## Local verification

The repository declares and certifies Rust 1.95.

```sh
cp .env.example .env
set -a; source .env; set +a
cargo fmt --all -- --check
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo run --locked
```

## Container and release

The multi-stage image uses a distroless non-root runtime (UID/GID `65532`) and contains only the release binary and runtime libraries.

```sh
docker build --tag canonical-api-server:local .
docker run --rm \
  --env CANONICAL_INTERNAL_AUTH_TOKEN=replace-with-at-least-32-random-bytes \
  --env SHARED_AUTH_VERIFY_URL=http://host.docker.internal:8120/shared-auth/auth/verify \
  --publish 8080:8080 \
  canonical-api-server:local
curl --fail http://127.0.0.1:8080/healthz
```

Application CI formats, tests, lints, builds, and smoke-tests the image. The Canonical monorepo is the intended deployable release authority and must publish this separately pinned API source as an immutable digest with provenance and an SBOM. GitOps consumes that digest rather than a mutable tag.

## Production gates outside this repository

- provision the exact Canonical PostgreSQL runtime/migration roles and apply the reviewed schema;
- inject Gemini, internal service, and Shared Auth configuration from the cluster secret store;
- deploy the Canonical Shared Auth realm and exact verification endpoint;
- certify direct bearer and web-BFF authentication, owner isolation, revocation, provider failure, restart recovery, list/retry, and WebSocket reconnect behavior;
- implement durable cross-replica leasing and stale-claim recovery under DEN-2599 before treating in-flight analysis as replica-failure resilient.
cargo fmt -- --check
cargo test --offline
```
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
