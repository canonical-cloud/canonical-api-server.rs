# Canonical quote API

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

```sh
cargo fmt -- --check
cargo test --offline
```
