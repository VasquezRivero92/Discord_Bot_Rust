# syntax=docker/dockerfile:1
#
# Frente de entrada de machupeek.com (binario `web`). Sin secretos en la imagen:
# toda la configuración llega por variables de entorno en tiempo de ejecución.

# ---------------------------------------------------------------- builder
FROM rust:1-bookworm AS builder

# aws-lc-sys (backend de rustls/jsonwebtoken) compila C con cmake.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake build-essential clang \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p web \
    && cp target/release/web /usr/local/bin/web

# ---------------------------------------------------------------- runtime
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin web

COPY --from=builder /usr/local/bin/web /usr/local/bin/web

# Los estáticos (build de public/ del legado) se montan aquí en solo lectura.
ENV PORT=15633 \
    STATIC_DIR=/srv/public \
    RUST_LOG=info

USER web
EXPOSE 15633

# GET http://127.0.0.1:$PORT/api/ping con el propio binario (la imagen no trae curl).
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/web", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/web"]
