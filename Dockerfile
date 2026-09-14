# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.96.0
ARG NODE_VERSION=24.8.0
ARG REVISION=unknown

FROM node:${NODE_VERSION}-bookworm-slim AS web-build

ENV COREPACK_HOME=/corepack
WORKDIR /workspace
RUN corepack enable && corepack prepare pnpm@10.34.3 --activate
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml .npmrc ./
COPY web/package.json web/package.json
RUN --mount=type=cache,id=faktory-pnpm,target=/pnpm/store \
    pnpm config set store-dir /pnpm/store && pnpm install --frozen-lockfile
COPY proto/gen/ts proto/gen/ts
COPY web web
RUN pnpm run web:build

FROM rust:${RUST_VERSION}-slim-bookworm AS rust-build

ARG TARGETARCH
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
WORKDIR /workspace
RUN apt-get update && \
    apt-get install -y --no-install-recommends build-essential ca-certificates git pkg-config && \
    rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN --mount=type=cache,id=faktory-${TARGETARCH}-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=faktory-${TARGETARCH}-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=faktory-${TARGETARCH}-cargo-target,target=/workspace/target \
    --mount=type=secret,id=gitconfig,target=/root/.gitconfig,required=false \
    --mount=type=secret,id=git-credentials,target=/root/.git-credentials,required=false \
    cargo build --locked --release -p faktory-server && \
    cp /workspace/target/release/faktory-server /usr/local/bin/faktory-server

FROM mambaorg/micromamba:2.3.3@sha256:800e7ade3ffe29c9a9ac2026163131495f8197c3852e572c5835beb4e8a33cd6 AS python-build

ARG TARGETARCH
USER root
ENV MAMBA_ROOT_PREFIX=/tmp/micromamba
COPY renderer/conda /tmp/conda
RUN case "${TARGETARCH}" in \
      amd64) lock=/tmp/conda/conda-linux-64.lock ;; \
      arm64) lock=/tmp/conda/conda-linux-aarch64.lock ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    micromamba create --yes --prefix /opt/faktory/env --file "${lock}" && \
    micromamba clean --all --yes && \
    rm -rf /tmp/conda /tmp/micromamba

FROM debian:bookworm-slim AS runtime

ARG REVISION
LABEL org.opencontainers.image.source="https://git.holdenitdown.net/rfhold/faktory" \
      org.opencontainers.image.revision="${REVISION}"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      ca-certificates \
      libfontconfig1 \
      libfreetype6 \
      libgl1 \
      libglib2.0-0 \
      libx11-6 \
      libxext6 \
      libxrender1 && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 65532 faktory && \
    useradd --uid 65532 --gid 65532 --home-dir /nonexistent --no-create-home --shell /usr/sbin/nologin faktory

ENV HOME=/tmp \
    PATH=/opt/faktory/env/bin:/usr/local/bin:/usr/bin:/bin \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONPATH=/opt/faktory \
    FAKTORY_STATIC_DIR=/opt/faktory/web \
    FAKTORY_RENDER_COMMAND_JSON='["/opt/faktory/env/bin/python","-m","renderer"]'
WORKDIR /opt/faktory

COPY --from=rust-build /usr/local/bin/faktory-server /usr/local/bin/faktory-server
COPY --from=python-build /opt/faktory/env /opt/faktory/env
COPY --from=web-build /workspace/web/dist /opt/faktory/web
COPY renderer /opt/faktory/renderer

EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/faktory-server"]
