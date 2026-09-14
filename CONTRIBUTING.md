# Contributing

Read [the documentation index](docs/README.md) and the nearest contract before changing behavior. Keep source and generated protocol bindings in the same change.

## Toolchains

- Rust `1.96.0` with edition 2024.
- Buf v2.
- Node.js with pnpm `10.34.3`.
- Python `>=3.12,<3.13` managed by uv for native renderer development and tests.
- conda-lock `3.0.4` and micromamba `2.3.3` for the production image's Linux AMD64 and ARM64 renderer environments.
- Bun `1.3.5` for Pulumi declaration tests and deployment.
- Docker Engine with Docker Compose v2 or later for the integration environment.

## Generate Protocol Bindings

Install the local executables required by `buf.gen.yaml`, then run:

```bash
pnpm run generate:proto
```

Rust generation requires `protoc-gen-prost` and `protoc-gen-tonic`. TypeScript generation requires the workspace-installed `protoc-gen-es`; run `pnpm install` before generation. The workspace command normalizes the TypeScript generator's terminal whitespace. Generated Rust and TypeScript files are committed.

## Validate

Run from the repository root:

```bash
buf lint
cargo +1.96.0 fmt --all -- --check
cargo +1.96.0 check --workspace --all-targets
cargo +1.96.0 clippy --workspace --all-targets -- -D warnings
cargo +1.96.0 test --workspace --locked
pnpm install --frozen-lockfile
pnpm run typecheck
pnpm run web:build
pnpm run web:test
uv sync --frozen
uv run python -m unittest discover
cd infra/pulumi
bun install --frozen-lockfile
bun run build
bun run test
cd ../..
docker compose config --quiet
git diff --check
```

The preview and release pipelines run this complete sequence after the pinned Gitleaks scan passes.

## Releases

A release requires a committed remote `main` branch and an annotated, signed `vX.Y.Z` tag. The tag must point to a commit in `origin/main`. The version must equal both `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json`.

The release pipeline accepts only an exact stable semantic-version tag. It verifies the webhook SHA, tag signature, trusted signer, and `origin/main` ancestry. It publishes AMD64 and ARM64 images, promotes only the immutable manifest digest to `vX.Y.Z`, and deploys that digest to the Pulumi `prod` stack. It does not publish a floating alias.

## Renderer Environments

`pyproject.toml` and `uv.lock` remain authoritative for native development and tests. The production image instead installs the conda-forge packages in the committed explicit locks under `renderer/conda/`; each package URL includes a checksum, and the Docker build does not solve dependencies.

Regenerate both image locks from the repository root with the pinned conda-lock version:

```bash
uvx --from conda-lock==3.0.4 conda-lock lock \
  --file renderer/environment.yml \
  --kind explicit \
  --platform linux-64 \
  --platform linux-aarch64 \
  --filename-template 'renderer/conda/conda-{platform}.lock' \
  --without-cuda
```

Review all changed package URLs and checksums before committing regenerated locks. BuildKit maps `TARGETARCH=amd64` to `conda-linux-64.lock` and `TARGETARCH=arm64` to `conda-linux-aarch64.lock`; other architectures fail before installation.

## Local Compose Integration

Compose initializes Garage and Faktory PostgreSQL from named volumes. Faktory runs with exact `FAKTORY_AUTH_MODE=disabled`, so the SPA, gRPC-web, artifact, and MCP routes require no cookie or bearer token. The Docker build selects the native Linux AMD64 or ARM64 renderer lock automatically.

All fixed Compose credentials are local-development-only. Open `http://localhost:8080` directly; no sign-in or MCP OAuth flow is used locally. The Faktory port is published only on `127.0.0.1`. Never expose this unauthenticated topology beyond loopback:

```bash
docker compose build
docker compose up --wait --wait-timeout 180
curl --fail http://localhost:8080/health
curl --fail http://localhost:8080/ready
docker compose down --volumes --remove-orphans
```

Use `docker compose logs faktory garage garage-init postgres` to inspect startup failures. The cleanup command removes all local application and storage state.

Tests and checks prove local source and declaration consistency. Compose exercises only explicit disabled authentication mode; it does not exercise or prove production Authentik, hosted MCP OAuth, Ceph, ingress, TLS, or deployed behavior. When `FAKTORY_AUTH_MODE` is absent, the server defaults to production authentication and fails closed if its required production configuration is missing or invalid.
