# Pre-interest API and persistence foundation

Tracking: DEN-4058.

This branch contains validation and a declarative PostgreSQL candidate for Canonical pre-interest registration. It deliberately does not bind a public HTTP route yet.

Before `POST /v1/pre-interest-registrations` can be exposed, the same reviewed head must have generated TypeSpec/Protobuf/JSON Schema codecs, bounded request-body and anti-automation controls, durable Diesel/diesel-async writes, a SeaORM-compatible read projection with operation parity, reviewed namespace and grants, disposable PostgreSQL execution, separate alias/fingerprint/contact-encryption key paths, and exact-host canary evidence in `canonical-cloud-test`.

The origin must derive the accepted host from the trusted request. `user.canonical.plus` is limited to individual registration and `org.canonical.plus` to organization registration. Forwarded host, caller identity, risk classification, and network metadata supplied by the public body are never authoritative.

Raw contact values must not enter URLs, idempotency keys, logs, traces, metrics, Cloudflare headers, or error bodies. Email lookup and request binding use different server-held keyed derivations. A reused request ID with a different canonical request is a conflict. New, duplicate, and previously known contacts receive the same public acceptance shape.

The database source on this branch is a non-activated candidate outside the production namespace manifest. No migration, Cloudflare change, Kubernetes deployment, secret provisioning, provider message, quote creation, or production write is authorized by this branch.
