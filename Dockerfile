FROM rust:1.88-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && apt-get clean \
    && find /var/lib/apt/lists -mindepth 1 -delete
WORKDIR /app
COPY --from=build /build/target/release/canonical-api-server /usr/local/bin/canonical-api-server
COPY context ./context
USER 65532:65532
EXPOSE 8081
ENTRYPOINT ["canonical-api-server"]
