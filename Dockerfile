# syntax=docker/dockerfile:1.7

FROM rust:1.95.0-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY context ./context
COPY db ./db
COPY vendor ./vendor
RUN cargo build --locked --release --bin canonical-api-server

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder --chown=65532:65532 /workspace/target/release/canonical-api-server /app/canonical-api-server

EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/app/canonical-api-server", "serve"]
