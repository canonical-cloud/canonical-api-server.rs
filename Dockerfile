# syntax=docker/dockerfile:1.7

# Shared shell-free launcher, compiled for the target architecture.
FROM rust:1.90-bookworm AS launcher-build
WORKDIR /launcher-source
COPY docker/ores-launcher.rev ./ores-launcher.rev
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    grep -Eq '^[0-9a-f]{40}$' ores-launcher.rev \
    && test "$(wc -l < ores-launcher.rev)" -eq 1 \
    && cargo install --locked \
        --git https://github.com/ores-otel/ores.otel.log.git \
        --rev "$(cat ores-launcher.rev)" \
        --features launcher --bin ores-launcher --root /launcher \
        oresoftware-next-loggers \
    && strip /launcher/bin/ores-launcher

FROM rust:1.95.0-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY context ./context
COPY db ./db
RUN cargo build --locked --release --bin canonical-api-server

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app
COPY --from=builder --chown=65532:65532 /workspace/target/release/canonical-api-server /app/canonical-api-server
COPY --from=launcher-build --chmod=0555 /launcher/bin/ores-launcher /ores-launcher

EXPOSE 8080
USER 65532:65532
# --- sops: this final stage has no shell (distroless/scratch), so runtime
# decryption cannot run inside the container. Inject secrets HOST-SIDE at
# `docker run` instead — never at build, never as --build-arg:
#     just env-docker-run prod <image>        # decrypts env/enc/prod.env.enc
#                                             # and passes --env-file, no plaintext on disk
# or render a platform secret from the same ciphertext. See env/README.md.
# Keep the existing fixed `serve` command and argument-only override semantics.
ENTRYPOINT ["/ores-launcher", "/app/canonical-api-server", "serve"]
CMD []
