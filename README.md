# canonical-api-server.rs

Rust readiness-data REST, WebSocket, and signed-webhook boundary for
`api.canonical.plus`.

`app.canonical.plus` remains owned by `canonical-web-server.rs`. The web server
authenticates the browser with the Canonical Shared Auth realm, then calls this
service over a private or service-mesh path with a separately stored internal
token and the already introspected subject. Direct `api.canonical.plus` clients
present a Shared Auth bearer scoped to `canonical-plus-api`; the API
independently introspects it through the pinned official Shared Auth Rust client.
Internet clients cannot select their own `x-canonical-subject` or make an edge
identity projection authoritative.

## Package direction

```text
canonical-interfaces → canonical-lib-core → canonical-api-server.rs
```

The Zed dependency graph is declared in `.zpkg.toml`. Generated transport
contracts stay in `canonical-interfaces`; reusable quote validation and context
assembly belong in `canonical-lib-core` (the compatibility Rust crate remains
`canonical-lib`); this repository owns HTTP, WebSocket,
PostgreSQL persistence, and Gemini provider orchestration.

## Implemented quote flow

The application-level Markdown policy is compiled from
`context/quote-analysis.md`. Browser requests may omit `markdown_context`, and
any supplied value is ignored. Customer-specific facts still come from the
normalized intake fields and the owner-scoped `canonical_context` row.

1. `POST /api/v1/quotes` accepts the generated lowerCamelCase `QuoteRequest`,
   validates and normalizes its bounded questionnaire fields, and derives the
   owner only from the trusted identity projection.
2. PostgreSQL selects the authenticated owner's single active
   `canonical_context` row. A partial unique index makes that selection
   unambiguous; browser input cannot choose a context UUID or tenant.
3. The API stores immutable snapshots of the normalized request, application
   policy, and selected owner context before analysis begins.
4. Gemini receives bounded untrusted questionnaire/context input through a
   fixed Google origin with no redirects, bounded retries and responses,
   schema-constrained JSON, and a circuit breaker.
5. The public Quote v1 contract progresses through `queued`, `analyzing`, and
   then `ready` or `failed`. Private persistence uses `completed` for a
   successful model attempt and maps it to public `ready`.
6. REST remains authoritative. Owner-scoped WebSocket events are delivery
   hints, and clients recover durable state through REST after disconnects or
   process restarts.
7. A failed quote can be requeued through the owner-scoped retry endpoint.
8. Create and retry require an `Idempotency-Key`. Replaying the same owner,
   operation, and normalized input returns the original result; incompatible
   reuse fails with `409 idempotency_key_reused` and starts no second analysis.
9. Each accepted status transition can emit a signed, timestamped webhook with
   a stable `quote:{quoteId}:{sequence}` identifier. Receivers can reject stale
   deliveries and deduplicate retries without trusting arrival order.

The default model is `gemini-3.6-flash` and remains configurable through
`GEMINI_MODEL`. Requests use the Google API-key header, a fixed Google API
origin, disabled redirects, bounded responses, retries for transport/429/5xx
failures, and a small circuit breaker. Prompts, context, model output, internal
tokens, bearer tokens, and API keys are never written to application logs or
returned in public payloads.

The generated analysis is preliminary scoping assistance for human review. It
is not a certification, audit opinion, legal opinion, attestation, or guarantee.

## Outbound readiness webhooks

Set `CANONICAL_WEBHOOK_URL` and `CANONICAL_WEBHOOK_SECRET` together to deliver
`canonical.quote.status.changed` events. The endpoint must use HTTPS, except
for an HTTP loopback endpoint used in local development. URLs containing user
credentials, query parameters, or fragments are rejected so secrets do not
leak into configuration diagnostics or intermediary logs. The secret must
contain at least 32 bytes.

Every request carries:

- `x-canonical-webhook-id`: the stable event identifier;
- `x-canonical-webhook-timestamp`: a Unix timestamp;
- `x-canonical-webhook-signature`: `v1=` followed by the lowercase hex
  HMAC-SHA256 of `<timestamp>.<exact request body>`.

Receivers should verify the signature with a constant-time comparison before
parsing the body, enforce a narrow timestamp window, and persist the webhook ID
before applying an effect. Delivery uses a five-second timeout, does not follow
redirects, stops on terminal 4xx responses, and makes at most three bounded
attempts for transport, 408, 429, and 5xx failures. Payloads omit the trusted
subject and all model/context data.

This initial dispatcher is a best-effort status signal, like the WebSocket
stream; REST remains authoritative. It is not yet a durable customer callback
system. Before contractual webhook delivery, write the stable event into a
PostgreSQL outbox in the same transaction as the quote transition, lease it to
an idempotent worker, and retain delivery receipts and dead-letter state.

The API accepts at most four in-process analyses at a time, five accepted
submissions per subject per ten-minute window, and 500 accepted submissions
process-wide in that window. It rejects oversized request bodies, returns
`429` with `Retry-After` when the subject/global quota is exhausted, and
returns `503` before persistence when analysis capacity is full. Every API
response is marked `Cache-Control: no-store` and carries anti-sniffing,
anti-framing, no-referrer, and deny-all CSP headers. These admission controls
are deliberately a local safety valve; Cloudflare must also enforce a
distributed per-account/per-IP quota before traffic reaches this service.

Quote execution is still an in-process task after the durable `queued` state
is committed. Before selling production analysis or evidence workflows, move
execution to a durable, idempotent worker/outbox that leases pending work,
recovers abandoned `queued`/`analyzing` jobs after restart, and records retry
attempts. A local concurrency limit is not a substitute for that recovery path.

The generated analysis is a non-binding scope and price estimate. It is not a
certification, audit opinion, legal opinion, or guarantee of attestation.

## Routes

| Route | Purpose |
| --- | --- |
| `GET /healthz` | Liveness plus non-secret DB/Gemini/webhook configuration state |
| `POST /api/v1/quotes` | Validate, persist, and asynchronously analyze a quote |
| `GET /api/v1/quotes/{quote_id}` | Owner-scoped durable quote status/result |
| `POST /api/v1/quotes/{quote_id}/retry` | Idempotently requeue a failed quote |
| `GET /api/v1/quotes/{quote_id}/events` | Owner-scoped WebSocket status stream |

Cloudflare and the ingress must remove client-supplied `x-canonical-*` and
`x-auth-*` headers. The API must not be routed directly to an unrestricted
public origin. The origin accepts two non-overlapping authority paths:

- private web-BFF requests use the constant-time internal token plus the
  trusted projected subject; and
- direct API requests use an end-user bearer that is independently introspected
  for the exact `canonical-plus-api` audience with a separate service
  credential.

If either internal header is present, the request must satisfy the complete
internal path and cannot fall back to bearer introspection. Missing, inactive,
wrong-audience, malformed, or unavailable Shared Auth introspection fails with
the same bounded `401` response.

### Private web-to-API path

All quote routes require both:

- `x-canonical-internal-token`: a random service credential of at least 32
  bytes, compared in constant time;
- `x-canonical-subject`: the Shared Auth subject projected only by the trusted
  Canonical web tier.

The API does not accept `x-auth-*`, `x-canonical-user-id`,
`x-canonical-user-email`, `x-canonical-service-token`, or request-body
owner/tenant fields as identity authority. Cloudflare, ingress, and the web tier
must remove caller-supplied internal and identity headers before forwarding.

### Direct Shared Auth API path

Direct callers send `Authorization: Bearer <token>`. The Cloudflare Worker
validates the distinct API audience before proxying, then the Rust service uses
`shared-auth-client` to call protected introspection with
`SHARED_AUTH_INTROSPECT_SECRET` and repeats the audience/active-subject decision.
The service credential is sent only to Shared Auth; it is never forwarded to a
product caller, placed in a request body, logged, or used as the end-user token.

The official Shared Auth Rust client is consumed directly from immutable commit
`cc57a85b276bee81ad94decc87df2f48d49cab9f`. Protected introspection sends the
strict `IntrospectionRequest` envelope with the exact API audience and either
`quotes:read` or `quotes:write`. The independent service credential is required
before token constraints are parsed, responses are capped at 64 KiB, redirects
are disabled by the client, and unknown future response fields remain forward
compatible. Hosted builds must receive approved read access to the private
dependency; private auth source is never copied into this public repository to
bypass that boundary.

Application logging uses the Ores Rust logger from immutable commit
`ca176fb6768a9750d262a536952268625ffd3a8a`, bridged into the existing JSON
tracing stream. The startup record contains fixed service metadata only; tokens,
credentials, database URLs, identity values, request payloads, and upstream
response bodies are excluded.

## Web-to-API data-plane modes

[`src/web_data_plane.rs`](src/web_data_plane.rs) is the typed, bounded policy
source for all four supported paths. Every request carries the product audience
and verified subject; transport service credentials remain separate and never
appear in the product envelope.

1. **Direct read-only database:** only reads are accepted, using exact role
   `canonical_cloud__quote__web_ro`, transaction read-only mode, forced RLS,
   bounded statements/locks, and a 1,000-row ceiling. The current deployment
   grants no direct tables, so this mode stays disabled until reviewed read-only
   views and grants exist; it can never be used for writes.
2. **Stateless HTTP:** the current default. It requires HTTPS (or explicit
   loopback/in-cluster development), a secret-store reference, no redirects,
   a two-second connect ceiling, a ten-second request ceiling, a 64 KiB request
   ceiling, and a 256 KiB response ceiling.
3. **Stateful mTLS/TCP:** TLS 1.3 with CA, client-certificate, and client-key
   references. Messages use one four-byte big-endian length prefix and reject
   empty, oversized, truncated, or trailing frames before JSON parsing.
4. **Durable NATS JetStream:** distinct request/status subjects, a durable
   consumer, transactional outbox/inbox/status tables, stable dedupe keys,
   bounded messages and acknowledgements, capped deliveries, and monotonic
   persisted status transitions through success or dead-letter state.

## Public wire boundary

Public JSON uses lowerCamelCase and generated types including:

- `QuoteRequest`;
- `QuoteSubmissionResponse`;
- `QuoteDetail` and `QuoteSummary`/`QuoteListResponse`;
- `QuoteRetryResponse`;
- `QuoteStatusEvent`;
- `QuoteEstimate` and `QuoteProblem`.

Public identifiers use `quoteId`; statuses are exactly `queued`, `analyzing`, `ready`, and `failed`. Model-attempt rows, persistence labels, raw prompts/responses, internal context identifiers, service credentials, and tenant internals are not exposed.

The idempotency key is admitted only after authentication and bounded request
validation. PostgreSQL serializes concurrent replays in the same owner-scoped
transaction with an advisory lock and operation ledger, compares the stored
normalized request, and emits the initial event only for the winning operation.

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

Run the same verification locally against a disposable PostgreSQL cluster:

```sh
python3 scripts/verify-declarative-db-contract.py
DPM_BIN=/path/to/dpm \
POSTGRES_ADMIN_URL=postgres://postgres@127.0.0.1:5432/postgres \
  scripts/test-declarative-postgres.sh
```

## Gemini

Set `GEMINI_API_KEY` through the Kubernetes secret store. When the key is
absent, health remains available and accepted development requests transition
to `failed` with the bounded code `gemini_not_configured`.

- `CANONICAL_INTERNAL_AUTH_TOKEN` — private web/API service credential;
- `DATABASE_URL` — owner-scoped PostgreSQL runtime connection;
- `GEMINI_API_KEY` — server-side Gemini credential;
- `SHARED_AUTH_BASE` — exact Shared Auth base URL;
- `SHARED_AUTH_INTROSPECT_SECRET` — independent protected-introspection
  credential with at least 32 random bytes;
- `SHARED_AUTH_AUDIENCE` — exact delegated API audience, defaulting to
  `canonical-plus-api`;
- `CANONICAL_WEBHOOK_URL` — optional signed status-event destination; and
- `CANONICAL_WEBHOOK_SECRET` — paired HMAC secret with at least 32 random bytes.

`GEMINI_MODEL` defaults to `gemini-3.6-flash`. `BIND_ADDRESS` defaults to
`0.0.0.0:8080`.
The accepted service credential setting and HTTP header are exclusively
`CANONICAL_INTERNAL_AUTH_TOKEN` and `x-canonical-internal-token`.

The response schema includes phased framework services, conservative USD fee
and duration ranges, assumptions, risks, missing information, and a required
non-binding disclaimer. When the Gemini key is absent, health remains available
and accepted development requests transition to `failed` with a bounded public
problem. No provider error body is returned or logged.

## Develop

```sh
cp .env.example .env
set -a; source .env; set +a
cargo fmt --all -- --check
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo run --locked
```

## Container and release

The multi-stage image uses a distroless non-root runtime (UID/GID `65532`) and
contains only the release binary and runtime libraries.

```sh
docker build --tag canonical-api-server:local .
docker run --rm \
  --env CANONICAL_INTERNAL_AUTH_TOKEN=replace-with-at-least-32-random-bytes \
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
- inject database, Gemini, and internal service credentials from the platform
  secret store;
- deploy behind the Canonical Shared Auth introspection boundary; and
- certify web-BFF authentication, owner isolation, revocation, provider
  failure, restart recovery, REST/list/retry recovery, and WebSocket reconnect
  behavior in Kubernetes staging; and
- implement durable cross-replica leasing and stale-claim recovery under
  DEN-2599 before treating in-flight analysis as replica-failure resilient.

## Environment secrets

Secrets live in this repo **encrypted** with [sops](https://github.com/getsops/sops) + [age](https://github.com/FiloSottile/age):
`env/enc/<dev|prod>.env.enc` is committed; `just env-use <name>` decrypts it to
`env/dec/<name>.env` (gitignored, mode 0600) and symlinks `./.env` to it. The
Nix dev shell provides the tooling, `just env-audit` runs keyless in CI, and
containers decrypt at `docker run` — never at build. See [`env/README.md`](env/README.md).
