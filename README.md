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
8. `POST /api/v1/quotes` accepts an optional UUID `Idempotency-Key`. Replaying
   the same owner and normalized request returns the original quote; reusing
   the key for different input fails with `409 idempotency_key_reused` and does
   not start a second analysis.

The default model is `gemini-3.6-pro` and remains configurable through
`GEMINI_MODEL`. Requests use the Google API-key header, a fixed Google API
origin, disabled redirects, bounded responses, retries for transport/429/5xx
failures, and a small circuit breaker. Prompts, context, model output, internal
tokens, bearer tokens, and API keys are never written to application logs or
returned in public payloads.

The generated analysis is preliminary scoping assistance for human review. It
is not a certification, audit opinion, legal opinion, attestation, or guarantee.

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

Cloudflare and the ingress must remove client-supplied `x-canonical-*` and
`x-auth-*` headers. The API must not be routed directly to an unrestricted
public origin.

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

The idempotency UUID becomes the quote identifier only after authentication and
bounded request validation. PostgreSQL resolves concurrent replays in the same
owner-scoped transaction with `ON CONFLICT DO NOTHING`, compares the stored
normalized request, and emits the initial event only for the winning insert.

## PostgreSQL ownership and namespace

The quote data plane uses one dedicated PostgreSQL namespace:

```text
schema:   canonical_cloud__quote
migrator: canonical_cloud__quote__migrator
API:      canonical_cloud__quote__api_rw
web:      canonical_cloud__quote__web_ro
```

The API role is a non-owner, non-superuser, non-`BYPASSRLS` login with only the
explicit DML grants in `db/grants.sql`. The web role deliberately has no direct
table access; the web service uses the authenticated API boundary. Only the
migrator owns the schema and performs DDL.

The role bootstrap pins `search_path` to
`pg_catalog,canonical_cloud__quote`, revokes `CREATE` on `public`, applies
bounded statement/lock/idle-transaction timeouts, and makes the web role
transaction-read-only. Runtime operations still set `app.current_subject`
inside each transaction, and every business query includes an explicit owner
predicate. All four tables use forced row-level security.

`DATABASE_URL` is optional only for local routing tests. Without it, quote state
is labelled `memory-only` and restart recovery is unavailable. Production must
use the `canonical_cloud__quote__api_rw` identity. The migration URL must be
held only by a protected migration job and never injected into the API process.

Within that namespace, `db/schema.sql` defines owner-scoped
`canonical_context`, durable `canonical_quote` request/context/result snapshots,
append-only `canonical_quote_event` rows, and private provider metadata in
`canonical_model_attempt`.

## Declarative migration workflow

`db/schema.sql` is the desired state, not an imperative migration history. It
contains no credentials, role creation, grants, transaction wrapper, or
`IF NOT EXISTS` escape hatch. The exact source digest and role mapping are
recorded in `db/namespace.json`.

The reviewed sequence is:

```sh
# 1. DBA/platform bootstrap: roles, namespace owner, search_path, timeouts.
psql "$POSTGRES_ADMIN_URL" -v ON_ERROR_STOP=1 -f db/bootstrap.sql

# 2. Prove the migration against a throwaway shadow database.
dpm verify \
  --source-sql db/schema.sql \
  --target "$MIGRATION_DATABASE_URL" \
  --shadow "$POSTGRES_ADMIN_URL"

# 3. Apply the reviewed declarative plan through the migrator identity.
dpm apply \
  --source-sql db/schema.sql \
  --target "$MIGRATION_DATABASE_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --yes

# 4. Reconcile the explicit runtime grants after every schema apply.
psql "$POSTGRES_ADMIN_URL" -v ON_ERROR_STOP=1 -f db/grants.sql

# 5. Require an empty post-apply plan.
dpm diff \
  --source-sql db/schema.sql \
  --target "$MIGRATION_DATABASE_URL" \
  --shadow "$POSTGRES_ADMIN_URL" \
  --fail-on-diff
```

Destructive drift is never silently repaired. `dpm apply` must remain blocked
unless the reviewed command explicitly supplies the destructive consent flags.
The protected PostgreSQL 17 CI lane proves:

- role and ownership separation;
- no application object in `public`;
- forced RLS and cross-owner isolation;
- rejection of caller-forged ownership;
- no direct web-table access;
- no API DDL capability;
- idempotent replay with data preservation;
- out-of-band drift detection;
- destructive-change gating; and
- final shadow-replay convergence.

Run the same certification locally against a disposable PostgreSQL cluster:

```sh
python3 scripts/verify-declarative-db-contract.py
DPM_BIN=/path/to/dpm \
POSTGRES_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres \
  scripts/test-declarative-postgres.sh
```

## Configuration

Required in production:

- `CANONICAL_INTERNAL_AUTH_TOKEN` — private web/API service credential;
- `SHARED_AUTH_VERIFY_URL` — exact `/shared-auth/auth/verify` endpoint for direct bearer verification;
- `DATABASE_URL` — owner-scoped PostgreSQL runtime connection;
- `GEMINI_API_KEY` — server-side Gemini credential.

`GEMINI_MODEL` defaults to `gemini-3.6-pro`. `BIND_ADDRESS` defaults to
`0.0.0.0:8080`.
`CANONICAL_WEB_SERVICE_TOKEN` remains a temporary configuration alias for the
existing Kubernetes secret mapping; new deployments should use
`CANONICAL_INTERNAL_AUTH_TOKEN`. The accepted HTTP header remains only
`x-canonical-internal-token`.

The response schema includes phased framework services, conservative USD fee
and duration ranges, assumptions, risks, missing information, and a required
non-binding disclaimer. When the Gemini key is absent, health remains available
and accepted development requests transition to `failed` with a bounded public
problem. No provider error body is returned or logged.

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

Application CI formats, tests, lints, builds, and smoke-tests the image. On
`main`, `.github/workflows/release.yml` publishes immutable `main` and
commit-SHA tags to GHCR with provenance and an SBOM, then records the image
digest as a workflow artifact. The Canonical monorepo and GitOps configuration
must consume the immutable digest rather than a mutable tag.

## Production gates outside this repository

- verify the exact Canonical database, migration identity, runtime identity,
  namespace ownership, backups, restore point, and starting schema before any
  production apply;
- reconcile any owner that currently has multiple active context rows before
  enforcing the partial unique index;
- inject Gemini, internal service, and Shared Auth configuration from the cluster secret store;
- deploy the Canonical Shared Auth realm and exact verification endpoint;
- certify direct bearer and web-BFF authentication, owner isolation, revocation,
  provider failure, restart recovery, REST/list/retry recovery, and WebSocket
  reconnect behavior in Kubernetes staging; and
- implement durable cross-replica leasing and stale-claim recovery under DEN-2599 before treating in-flight analysis as replica-failure resilient.
