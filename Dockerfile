# syntax=docker/dockerfile:1.7

FROM rust:1.95.0-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY context ./context
COPY db ./db
RUN cargo build --locked --release --bin canonical-api-server

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
# ores-otel: in-process OTLP to the cluster collector. The *-sidecar.rs image is a separate loopback helper on 127.0.0.1:9090 — do not EXPOSE 4317/4318 or 9090.
ENV OTEL_SERVICE_NAME=canonical-api-server \
    OTEL_EXPORTER_OTLP_ENDPOINT=http://dd-otel-collector.observability.svc.cluster.local:4318 \
    RUST_LOG=info
ENTRYPOINT ["/app/canonical-api-server", "serve"]
