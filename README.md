# canonical-api-server.rs

Rust REST and WebSocket boundary for `api.canonical.plus`.

`app.canonical.plus` remains owned by `canonical-web-server.rs`. The web server
authenticates the browser with the Canonical Shared Auth realm, then calls this
service over a private or service-mesh path with a separately stored internal
token and the already introspected subject. Internet clients cannot select their
own `x-canonical-subject`.

## Package direction

```text
canonical-interfaces → canonical-lib → canonical-api-server.rs
```

The Zed dependency graph is declared in `.zpkg.toml`. Generated transport
contracts stay in `canonical-interfaces`; reusable quote validation and context
assembly belong in `canonical-lib`; this repository owns HTTP, WebSocket,
PostgreSQL persistence, and Gemini provider orchestration.

## Implemented quote flow

The application-level Markdown policy is compiled from
`context/quote-analysis.md`. Browser requests may omit `markdown_context`, and
any supplied value is ignored. Customer-specific facts still come from the
normalized intake fields and the owner-scoped `canonical_context` row.

1. `POST /v1/quotes` validates and normalizes a bounded request for SOC 2,
   HIPAA, NIST CSF, NIST 800-53, ISO 27001, PCI DSS, FedRAMP, or GDPR.
2. PostgreSQL selects the authenticated owner's single active
   `canonical_context` row. A partial unique index makes that selection
   unambiguous; browser input cannot choose a context UUID.
3. The API stores immutable snapshots of the application Markdown, selected
   database context, and normalized request before starting analysis.
4. Gemini receives those bounded inputs under explicit untrusted-data
   delimiters and returns schema-constrained JSON.
5. Durable quote state and append-only events transition from `queued` to
   `analyzing`, then `completed` or `failed`.
6. REST lookup and the WebSocket stream are owner scoped. WebSocket broadcasts
   are hints; PostgreSQL remains authoritative.

The default model is `gemini-3.6-flash` and remains configurable through
`GEMINI_MODEL`. Requests use the Google API-key header, a fixed Google API
origin, disabled redirects, bounded responses, retries for transport/429/5xx
failures, and a small circuit breaker. Prompts, context, model output, internal
tokens, and API keys are never written to application logs.

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
| `GET /healthz` | Liveness plus non-secret DB/Gemini configuration state |
| `POST /v1/quotes` | Validate, persist, and asynchronously analyze a quote |
| `GET /v1/quotes/{quote_id}` | Owner-scoped durable quote status/result |
| `GET /v1/quotes/{quote_id}/events` | Owner-scoped WebSocket status stream |

All quote routes require both:

- `x-canonical-internal-token`: a random service credential of at least 32
  bytes, compared in constant time;
- `x-canonical-subject`: the Shared Auth subject projected only by the trusted
  Canonical web tier.

Cloudflare and the ingress must remove client-supplied `x-canonical-*` and
`x-auth-*` headers. The API must not be routed directly to an unrestricted
public origin.

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

## Gemini

Set `GEMINI_API_KEY` through the Kubernetes secret store. When the key is
absent, health remains available and accepted development requests transition
to `failed` with the bounded code `gemini_not_configured`.

The response schema includes phased framework services, conservative USD fee
and duration ranges, assumptions, risks, missing information, and a required
non-binding disclaimer. No provider error body is returned to clients or
logged.

## Develop

```sh
cp .env.example .env
set -a; source .env; set +a
cargo fmt --all --check
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo run
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
digest as a workflow artifact. Canonical GitOps configuration must consume the
digest rather than a mutable tag.

## Production gates outside this repository

- verify the exact Canonical database, migration identity, runtime identity,
  namespace ownership, backups, restore point, and starting schema before any
  production apply;
- reconcile any owner that currently has multiple active context rows before
  enforcing the partial unique index;
- inject database, Gemini, and internal service credentials from the platform
  secret store;
- deploy behind the Canonical Shared Auth introspection boundary; and
- certify owner isolation, revocation, provider failure, restart recovery, REST
  recovery, and WebSocket reconnect behavior in Kubernetes staging.
