# Graceful-shutdown validation

This branch implements one coordinated lifecycle for HTTP, WebSocket, readiness, and background quote-analysis work.

## Required behavior

- SIGTERM and non-interactive SIGINT start graceful drain with one signal.
- In a TTY, the first Ctrl-C starts graceful drain; a second Ctrl-C forces shutdown.
- After the first interactive Ctrl-C, Ctrl-D can replace the second Ctrl-C.
- Closed non-interactive stdin is ignored.
- Readiness becomes unavailable before the listener stops accepting work; liveness remains available.
- Connections and accepted background jobs drain until the configured deadline.
- A second interactive input or the deadline cancels remaining quote work and closes active connections immediately.
- Structured shutdown logs include phase, trigger, force reason, signal count, TTY state, elapsed time, active connections, and outcome.

## Exact-source validation

The rollout workflow validated the product source before removing its migration scaffolding:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

This file is intentionally committed after cleanup so the pull-request head is human-authored and contains only permanent product or documentation changes.
