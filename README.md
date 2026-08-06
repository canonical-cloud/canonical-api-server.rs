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
4. Gemini receives those three bounded inputs under explicit untrusted-data
   delimiters and returns schema-constrained JSON.
5. Durable quote state and append-only events transition from `queued` to
   `analyzing`, then `completed` or `failed`.
6. REST lookup and the WebSocket stream are owner scoped. WebSocket broadcasts
   are hints; a restarted process recovers the current quote from PostgreSQL.

The default model is `gemini-3.1-pro-preview` and remains configurable through
`GEMINI_MODEL`. Requests use the Google API-key header, a fixed Google API
origin, disabled redirects, bounded responses, retries for transport/429/5xx
failures, and a small circuit breaker. Prompts, context, model output, internal
tokens, and API keys are never written to application logs.

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
`x-auth-*` headers. The API should not be routed directly to an unrestricted
public origin.

## PostgreSQL

Apply [`db/schema.sql`](db/schema.sql) through the migration identity, not the
runtime service login. The schema provides:

- `canonical_context`, with at most one active row per owner;
- `canonical_quote` with immutable input snapshots and the structured result;
- append-only `canonical_quote_event`;
- provider metadata in `canonical_model_attempt`;
- forced row-level security based on the transaction-local
  `app.current_subject`.

The runtime login must be non-owner, non-superuser, non-`BYPASSRLS`, and have
only the grants documented at the end of the schema. Every operation sets the
subject inside the same transaction and still includes an explicit owner
predicate.

`DATABASE_URL` is optional only for local routing tests. Without it, quote state
is labelled `memory-only` and restart recovery is unavailable. Production must
configure PostgreSQL.

## Gemini

Set `GEMINI_API_KEY` through the Kubernetes secret store. When the key is
absent, health remains available and accepted development requests transition
to `failed` with the bounded code `gemini_not_configured`.

The response schema includes:

- phased framework services;
- conservative USD fee and duration ranges;
- assumptions, risks, and missing information;
- a required non-binding disclaimer.

No provider error body is returned to clients or logged.

## Develop

```sh
cp .env.example .env
set -a; source .env; set +a
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
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
`main`, `.github/workflows/release.yml` publishes immutable `main` and commit-SHA
tags to GHCR with provenance and an SBOM, then records the image digest as a
workflow artifact. Canonical GitOps configuration must consume the digest rather
than a mutable tag.

## Production gates outside this repository

- provision the exact Canonical PostgreSQL runtime/migration roles, reconcile
  any owner that currently has multiple active context rows, and apply the
  reviewed schema;
- inject the Gemini key and internal service token from the cluster secret
  store;
- deploy behind the Canonical Shared Auth introspection boundary;
- certify owner isolation, revocation, provider failure, restart recovery, and
  WebSocket reconnect behavior in the Kubernetes staging environment.
