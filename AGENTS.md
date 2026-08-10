# Agent guidance

This repository owns the Rust REST and WebSocket boundary for
`api.canonical.plus`. Keep HTTP/authentication/persistence orchestration here,
transport contracts in `canonical-interfaces`, and reusable domain rules in
`canonical-lib`.

## Required checks

Run before proposing a merge:

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

Do not weaken owner isolation, secret redaction, bounded request validation, or
the explicit production gates documented in `README.md`.

## Command safety

Prefer read-only inspection before mutation. Ordinary file edits, `git add`,
`git commit`, `git mv`, and intentional `git rm` are allowed when they directly
serve the reviewed change. Never force-push, rewrite shared history, delete
branches/tags, run destructive resets/cleans, expose credentials, or change
repository visibility or deployment state without an explicit reviewed task.

Keep credentials out of source, logs, fixtures, issue text, and pull-request
bodies. Use placeholders in `.env.example` and least-privilege identities in
runtime configuration.
