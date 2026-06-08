# OCI image carrying the native `fossil` + `fossil-mcp` binaries (DuckDB bundled).
# Published to ghcr.io/kanzo-tech/fossil by .github/workflows/fossil-image.yml on
# release tags. keasy's server image does `COPY --from=ghcr.io/kanzo-tech/fossil:<tag>`
# instead of compiling this sibling source — that is the producer→consumer seam.

FROM rust:1.90-bookworm AS builder

# clang/libclang: DuckDB (bundled into the binaries) needs them to build.
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --release -p fossil-cli -p fossil-mcp

# ── runtime ───────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

# DuckDB looks for CA certs at the CentOS/RHEL path when fetching httpfs/azure
# extensions + reading cloud sources; symlink it (same as the keasy runtime).
RUN mkdir -p /etc/pki/tls/certs \
    && ln -s /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt

COPY --from=builder /src/target/release/fossil /usr/local/bin/fossil
COPY --from=builder /src/target/release/fossil-mcp /usr/local/bin/fossil-mcp

ENV FOSSIL_BIN=/usr/local/bin/fossil
ENV FOSSIL_MCP_BIN=/usr/local/bin/fossil-mcp
ENTRYPOINT ["fossil"]
