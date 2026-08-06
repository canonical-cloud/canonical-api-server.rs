# canonical-api-server.rs

Rust REST and WebSocket boundary for `api.canonical.plus`.

`app.canonical.plus` remains owned by `canonical-web-server.rs`. Browser traffic is authenticated by the Canonical Shared Auth realm at the web origin and reaches this service over a private path with a dedicated internal token plus the already verified subject. Direct SDK and CLI traffic uses a short-lived Shared Auth bearer token that this API independently verifies through the exact Shared Auth verification endpoint.

## Contract authority

The public quote v1 wire contract is generated from `canonical-cloud/canonical-interfaces` at immutable revision `3415d6d97721b18ee6734659d73a95ff3d35a151`.

```text
canonical-interfaces → canonical-lib → canonical-api-server.rs
```

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

## Develop

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
