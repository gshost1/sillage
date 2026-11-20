# syntax=docker/dockerfile:1
#
# sillage MCP server (Streamable HTTP on :8765).
#
#   docker build -t sillage .
#   export SILLAGE_MCP_TOKEN="$(openssl rand -hex 32)"
#   docker run --rm -p 8765:8765 -e SILLAGE_MCP_TOKEN="$SILLAGE_MCP_TOKEN" -v "$PWD:/workspace:ro" sillage
#
# The default command binds 0.0.0.0, so SILLAGE_MCP_TOKEN is REQUIRED: without
# it (or with a blank one) the server refuses to start. On a trusted network
# only, opt out explicitly:
#   docker run ... sillage serve --host 0.0.0.0 --port 8765 --allow-unauthenticated
#
# If you change --port, also pass -e PORT=<port> so the HEALTHCHECK follows.
#
# With no --root / --corpus, `serve` indexes its working directory
# (/workspace). Pass corpora explicitly to serve several:
#   docker run ... sillage serve --host 0.0.0.0 --port 8765 \
#     --corpus api=/workspace/api --corpus lib=/indexes/lib.sil

# The workspace MSRV (Cargo.toml `rust-version`).
ARG RUST_VERSION=1.90
FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock corpus.toml ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sillage-cli \
 && install -m 0755 target/release/sillage /usr/local/bin/sillage

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system sillage \
 && useradd --system --gid sillage --home-dir /nonexistent --shell /usr/sbin/nologin sillage \
 && mkdir -p /workspace \
 && chown sillage:sillage /workspace
COPY --from=builder /usr/local/bin/sillage /usr/local/bin/sillage
USER sillage
WORKDIR /workspace
ENV PORT=8765
EXPOSE 8765
# Indexing a large /workspace can take a while before the port opens.
HEALTHCHECK --interval=30s --timeout=3s --start-period=120s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT:-8765}/healthz" || exit 1
ENTRYPOINT ["sillage"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8765"]
