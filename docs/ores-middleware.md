# Shared ORES middleware and rate-limit audit

The Canonical API installs `ORESoftware/ores-middleware` through `src/http_middleware.rs`, pinned to reviewed commit `ec2327b079b88ac8fe21c3a331fe414735e2e716`.

The composition module exposes `MIDDLEWARE_ORDER`, validates the reviewed ordering invariants, and wraps the Axum router before it enters the repository's existing `shutdown::serve` lifecycle. Graceful shutdown, readiness, Shared Auth, Gemini, webhook, persistence, and service-owned authorization remain unchanged.

The live boundary uses `frameworks::axum_audit::install_from_env`. That adapter forcibly disables the shared rate-limit decision before constructing the stack, so this rollout cannot consume or grant a second quota through an environment mistake.

Production enforcement requires a separate pull request after route budgets, trusted-proxy CIDRs, strict coordinator capacity, reconnect behavior, failure modes, and low-cardinality telemetry are reviewed. Raw IP addresses, email addresses, authentication subjects, tenant identifiers, tokens, and request payloads must never appear in limiter keys, logs, traces, or metric labels.

TypeSpec and JSON Schema/OpenAPI remain independent peer contract authorities in the middleware repository. Authority or generated-artifact drift fails closed.

The generic rollout helper was not retained because its call-site parser treated the repository-specific three-argument `shutdown::serve` function as a direct Axum server call and produced invalid syntax. The repository-specific composition preserves that shutdown API instead of selecting either implementation wholesale.
