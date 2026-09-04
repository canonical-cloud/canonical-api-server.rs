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

1. The web BFF resolves the caller's verified email and phone from Shared Auth.
   `POST /v1/quote-contact-selections` requires explicit confirmation of both
   and stores them in authenticated-encryption envelopes. Raw browser contact
   values are never verification authority.
2. `POST /v1/quotes` validates the canonical lower-camel-case questionnaire and
   atomically consumes the short-lived, owner-bound contact selection.
3. The same PostgreSQL transaction stores immutable revision 1, the selected
   owner context snapshot, an append-only queued event, a durable analysis job,
   a revocable 256-bit access capability expiring in 25 days, and a deduplicated
   SMS outbox row before returning `202`.
4. A separately credentialed worker leases jobs with `FOR UPDATE SKIP LOCKED`.
   Closing the browser or restarting either process does not lose accepted work;
   an expired lease is reclaimed automatically.
5. Gemini receives bounded request and context inputs under explicit
   untrusted-data delimiters and returns schema-constrained JSON. Durable state
   transitions from `queued` to `analyzing`, then `ready` or `failed`.
6. The notification worker decrypts a destination and capability only in memory
   and sends the permalink through Twilio Messaging. Shared Auth continues to
   use Twilio Verify exclusively for OTPs.
7. `POST /v1/quotes/{quote_id}/submissions` appends revision `n + 1` after the
   current revision reaches a terminal state. Earlier inputs and results are not
   mutated. The same quote link remains scoped to that stable quote.
8. REST lookup and WebSocket streams are owner scoped. Broadcasts are hints;
   PostgreSQL remains authoritative after reconnect or restart.

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
| `POST /v1/quote-contact-selections` | Confirm the BFF-asserted verified email and phone and mint a 15-minute selection |
| `POST /v1/quotes` | Validate, durably enqueue, and return without waiting for analysis |
| `GET /v1/quotes/{quote_id}` | Owner-scoped durable quote status/result |
| `POST /v1/quotes/{quote_id}/submissions` | Append and enqueue an edited immutable revision |
| `GET /v1/quotes/{quote_id}/events` | Owner-scoped WebSocket status stream |
| `POST /v1/quote-links/redeem` | Internal BFF-only capability exchange used by `/q/{capability}` |

All quote routes require both:

- `x-canonical-internal-token`: a random service credential of at least 32
  bytes, compared in constant time;
- `x-canonical-subject`: the Shared Auth subject projected only by the trusted
  Canonical web tier.

Contact-selection calls additionally require `x-canonical-verified-email` and
`x-canonical-verified-phone`. The BFF sets these only from the protected Shared
Auth verified-contact response. All four headers are marked sensitive in HTTP
middleware and must be stripped from public ingress requests.

Cloudflare and the ingress must remove client-supplied `x-canonical-*` and
`x-auth-*` headers. The API should not be routed directly to an unrestricted
public origin.

## PostgreSQL

Apply [`db/schema.sql`](db/schema.sql) through the migration identity, not the
runtime service login. The schema provides:

- `canonical_context`, with at most one active row per owner;
- encrypted, explicit-use `canonical_quote_contact_selection` rows;
- stable `canonical_quote` headers and immutable `canonical_quote_revision`
  inputs, context snapshots, and results;
- append-only, revision-scoped `canonical_quote_event` rows;
- recoverable leased work in `canonical_quote_job`;
- hashed and encrypted capabilities in `canonical_quote_access_link`;
- deduplicated product messages in `canonical_notification_outbox`;
- provider metadata in `canonical_model_attempt`;
- forced row-level security based on the transaction-local
  `app.current_subject`.

The runtime login must be non-owner, non-superuser, non-`BYPASSRLS`, and have
only the grants documented at the end of the schema. Every operation sets the
subject inside the same transaction and still includes an explicit owner
predicate.

`DATABASE_URL` is optional only for local routing tests. Without it, quote state
is labelled `memory-only` and restart recovery and link redemption are
unavailable. Production must also configure `QUOTE_WORKER_DATABASE_URL` with the
distinct `canonical_quote_worker` login. If the worker is absent, submissions
remain safely queued; the HTTP process does not fall back to process-local work.

`QUOTE_DATA_ENCRYPTION_KEY` is required with PostgreSQL and must decode from
unpadded base64url to exactly 32 random bytes. It encrypts contact destinations
and the plaintext copy of the access capability used by the SMS worker. The
database lookup value is a SHA-256 digest; neither plaintext value is logged.

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
- inject the quote encryption key and Twilio Messaging credentials, configure
  the HTTPS `QUOTE_LINK_BASE_URL`, and deploy the distinct worker database
  identity;
- deploy behind the Canonical Shared Auth introspection boundary;
- certify owner isolation, OTP/contact assertion handling, 25-day expiry,
  capability revocation, outbox deduplication, provider failure, lease recovery,
  edit/resubmit history, and WebSocket reconnect behavior in staging.
