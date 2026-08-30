# syntax=docker/dockerfile:1.7

FROM rust:1.95.0-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY context ./context
COPY db ./db
# The private canonical-lib-core dependency is fetched with the ephemeral
# BuildKit secret only. GIT_CONFIG_COUNT keeps the credential out of image
# layers, git config, build arguments, and command output.
RUN --mount=type=secret,id=github_token \
    token="$(cat /run/secrets/github_token)" && \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    GIT_CONFIG_COUNT=1 \
    GIT_CONFIG_KEY_0="url.https://x-access-token:${token}@github.com/.insteadOf" \
    GIT_CONFIG_VALUE_0="https://github.com/" \
    cargo build --locked --release --bin canonical-api-server

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder --chown=65532:65532 /workspace/target/release/canonical-api-server /app/canonical-api-server

EXPOSE 8080
USER 65532:65532
# --- sops: this final stage has no shell (distroless/scratch), so runtime
# decryption cannot run inside the container. Inject secrets HOST-SIDE at
# `docker run` instead — never at build, never as --build-arg:
#     just env-docker-run prod <image>        # decrypts env/enc/prod.env.enc
#                                             # and passes --env-file, no plaintext on disk
# or render a platform secret from the same ciphertext. See env/README.md.
ENTRYPOINT ["/app/canonical-api-server", "serve"]
