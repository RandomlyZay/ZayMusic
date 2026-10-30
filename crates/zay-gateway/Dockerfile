# Built from the repository root (see .dockerignore for the exact context).
FROM rust:1.99.1-slim-trixie AS build
WORKDIR /src

# Workspace resolution needs every member manifest. Only the gateway is
# compiled, so the other members get empty stand-in sources (no GTK, UniFFI
# or Apple inputs), and dependencies build once in a cached layer.
COPY Cargo.toml Cargo.lock ./
COPY crates/zay-core/Cargo.toml crates/zay-core/
COPY apps/linux/Cargo.toml apps/linux/
COPY crates/zay-gateway/Cargo.toml crates/zay-gateway/
RUN mkdir -p crates/zay-core/src apps/linux/src crates/zay-gateway/src \
    && touch crates/zay-core/src/lib.rs crates/zay-core/src/bin.rs crates/zay-gateway/src/lib.rs \
    && echo 'fn main() {}' > apps/linux/src/main.rs \
    && echo 'fn main() {}' > crates/zay-gateway/src/main.rs \
    && cargo build --release --locked -p zay-gateway

COPY crates/zay-gateway crates/zay-gateway
# COPY keeps source mtimes, which can predate the stand-ins above.
RUN touch crates/zay-gateway/src/*.rs \
    && cargo build --release --locked -p zay-gateway --bin zay-gateway

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --no-create-home zay-gateway
COPY --from=build /src/target/release/zay-gateway /usr/local/bin/zay-gateway
WORKDIR /app
USER zay-gateway
ENV HOST=0.0.0.0 PORT=3000
EXPOSE 3000
CMD ["zay-gateway"]
