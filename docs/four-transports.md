# The four avenues between `canonical-web-server` and `canonical-api-server`

`canonical-cloud` follows the fleet-wide contract in
[`ORESoftware/ores-transport`](https://github.com/ORESoftware/ores-transport).
This document is the org-specific half: which slug, which variables, and the
one thing each repo still has to implement.

| # | Mode | Path | Reach for it when |
|---|------|------|-------------------|
| 1 | `direct_read` | `canonical-web-server` → Postgres, via `canonical-lib` | a read on the page's critical path |
| 2 | `http` | `canonical-web-server` → `canonical-api-server`, stateless | the default, and anything that writes |
| 3 | `tcp` | `canonical-web-server` ⇄ `canonical-api-server`, held open | chatty internal calls where the handshake dominates |
| 4 | `jet_stream` | `canonical-web-server` → NATS → `canonical-api-server` | work the caller does not need to watch finish |

All four carry the same envelope and land on the same handler, so a
`canonical-e2e` suite can drive one operation four ways and assert the answers
agree. That equivalence is the point; without it the extra avenues are three
untested code paths.

## Service identity

| | |
|---|---|
| Slug | `canonical` |
| Environment prefix | `CANONICAL_` |
| Request subject | `canonical.api.operation.v1` |
| Result subject | `canonical.api.operation-result.v1` |
| Durable consumer | `canonical-api-server-v1` |

The slug is asserted against the prefix by a unit test in both crates. They
drift in exactly one way — someone renames the service and updates one of
them — and the symptom is that the web server publishes to subjects the api
server does not consume while both processes report healthy.

## Environment

| Variable | Avenue | Absent means |
|----------|--------|--------------|
| `CANONICAL_READONLY_DATABASE_URL` | 1 | no direct reads |
| `CANONICAL_API_URL` | 2 | no stateless HTTP |
| `CANONICAL_API_MTLS_ADDR` | 3 | no stateful TCP |
| `CANONICAL_API_TLS_SERVER_NAME` | 3 | plaintext to a TLS-terminating mesh |
| `CANONICAL_WEB_CLIENT_CERT_FILE` | 3 | ” |
| `CANONICAL_WEB_CLIENT_KEY_FILE` | 3 | ” |
| `CANONICAL_API_CA_FILE` | 3 | ” |
| `CANONICAL_NATS_URL` | 4 | no asynchronous avenue |

A missing variable disables its avenue. A variable that is present and unsafe
fails startup: a plaintext `CANONICAL_API_URL` off loopback, a
`CANONICAL_NATS_URL` that is not `tls://`, or mutual TLS with two of its
four files set.

In-cluster, `CANONICAL_NATS_URL` points at the messaging service defined in
`ORESoftware/k8s-cluster` (`remote/argocd/messaging/nats.service.yaml`),
wrapped in TLS.

## What is still to do in this repo

The generated modules wire the transports. They deliberately do not invent
domain semantics, because only this repo knows what an operation is.

**In `canonical-web-server`** — implement `DirectReader` over `canonical-lib`:

```rust
use ores_transport::{DirectReader, TransportError};

pub struct LibCoreReader { read: __CORE_CRATE_IDENT__::ReadContext }

#[async_trait::async_trait]
impl DirectReader<Operation, Projection> for LibCoreReader {
    async fn read(&self, op: &Operation, authorization: &str)
        -> Result<Projection, TransportError>
    {
        // Authorize first: skipping the api server skips a network hop,
        // not an access check.
        // Then build the query with canonical-lib's query builders.
    }

    fn serves(&self, op: &Operation) -> bool {
        matches!(op, Operation::Read { .. })   // reads only, on this avenue
    }
}

let gateway = four_transports::gateway_from_env(Arc::new(reader), tls).await?;
```

**In `canonical-api-server`** — implement `OperationHandler` once, and hand the same
`Arc` to the axum route, `serve_stateful` and `serve_asynchronous`. One
implementation shared by three avenues is what stops an operation meaning
different things on different wires.

## Two rules that are enforced, not just documented

**Avenue 1 is read-only twice over.** `canonical-lib` gates its write surface
behind a `read-write` cargo feature and migrations behind `migrate`. Depend on
it from `canonical-web-server` as:

```toml
canonical-lib = { git = "…", default-features = false, features = ["read-only"] }
```

so a mutating call is a *compile error*, not a code-review miss. Separately,
`CANONICAL_READONLY_DATABASE_URL` must name a Postgres role without
`INSERT`, `UPDATE`, `DELETE` or DDL. A mistake has to defeat both. Migrations
belong to `canonical-lib` and `declarative-migrations`, never to a web server.

**Every envelope is authenticated, not every connection.** A TCP connection
authenticated once at handshake outlives the token that opened it. The
credential rides inside the envelope and is re-checked on arrival, so a revoked
token stops working on the next frame rather than at the next reconnect.

## Ordering note

`ores-transport` is currently depended on by `branch = "main"`. Pin it to a rev
once it is pushed, matching how every other git dependency in this org is
pinned.
