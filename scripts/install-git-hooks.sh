#!/usr/bin/env sh
set -eu
cd "$(git rev-parse --show-toplevel)"
chmod +x .githooks/pre-commit .githooks/pre-push
git config core.hooksPath .githooks
