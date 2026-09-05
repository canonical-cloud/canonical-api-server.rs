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

## Repository-local Git worktrees

- Create or use a Git worktree only when the human operator explicitly authorizes it for the current task. Concurrency or a dirty checkout is not permission by itself.
- Put every authorized worktree at `<repository-root>/tmp/worktrees/<name>`; from the repository root, use `./tmp/worktrees/<name>`. Never place worktrees beside repositories or organization directories.
- Keep `tmp`, `temp`, `tmp/worktrees`, and `temp/worktrees` ignored in the repository-root `.gitignore`. Do not commit files from those directories.
- Relocate or remove a worktree only when the operator explicitly requests it. Before removal, preserve and publish intended changes, verify its commit is represented on the target branch, and confirm there are no tracked, untracked, ignored-sensitive, or in-use files that must survive. Remove it with `git worktree remove <path>` without `--force`; never delete a worktree directory with `rm`.
