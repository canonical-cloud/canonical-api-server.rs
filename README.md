# canonical-api-server.rs

Rust REST and WebSocket boundary for `https://api.canonical.plus`.
`https://app.canonical.plus` remains owned by `canonical-web-server.rs`; the
browser-facing server calls this API rather than sharing its database identity.

## Package direction

```text
canonical-interfaces → canonical-lib → canonical-api-server.rs
```

The Zed dependency graph is declared in `.zpkg.toml`. Generated transport
contracts stay in `canonical-interfaces`; quote validation and context assembly
stay in `canonical-lib`; this repository owns HTTP, WebSocket, persistence, and
model-provider orchestration.

## Current foundation

- Axum routes for health, quote submission, owner-scoped lookup, and quote event
  WebSockets;
- a required trusted-proxy token plus authenticated-subject projection, so the
  service cannot start in an accidentally anonymous mode;
- SeaORM/PostgreSQL connection plumbing;
- bounded Markdown/context-record identifiers for later owner-scoped loading
  from the `canonical_context` table;
- a configurable Gemini model, defaulting to `gemini-3.6-pro`;
- in-memory quote state only, deliberately labelled `memory-only` until the
  reviewed migration and repository layer land.

The trusted-proxy headers are an initial internal boundary, not the final
internet-facing authentication design. Before production, the API must verify
shared-auth JWTs or a sealed introspection response itself, Cloudflare must
strip client-supplied internal headers, and the database repository must enforce
owner/tenant predicates on every query.

## Routes

| Route | Purpose |
| --- | --- |
| `GET /healthz` | Liveness and configuration visibility without secrets |
| `POST /v1/quotes` | Validate and queue a quote request |
| `GET /v1/quotes/{quote_id}` | Owner-scoped quote status |
| `GET /v1/quotes/{quote_id}/events` | Authenticated WebSocket status stream |

## Develop

```sh
cp .env.example .env
set -a; source .env; set +a
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo run
```

## Production gates

1. add SeaORM entities/migrations for quote requests, context snapshots, model
   attempts, and append-only status events;
2. load the Markdown context file by reviewed digest and combine it with exactly
   one owner-scoped PostgreSQL context record through `canonical-lib`;
3. call Gemini through a timeout/retry/circuit-breaker boundary without logging
   prompts or regulated content;
4. replace the temporary trusted-proxy identity projection with shared-auth JWT
   verification and revocation/introspection handling;
5. certify REST/WebSocket owner isolation, CSRF rejection, revocation, model
   failure handling, and restart recovery in Kubernetes.
