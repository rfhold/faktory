# Contributing

Read [the documentation index](docs/README.md) and the nearest contract before changing behavior. Keep source and generated protocol bindings in the same change.

## Toolchains

- Rust `1.96.0` with edition 2024.
- Buf v2.
- Node.js with pnpm `10.34.3`.
- Chromium for visual renderer browser tests. The production worker image installs its pinned Playwright Chromium build.
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
pnpm run visual-renderer:typecheck
pnpm run visual-renderer:build
pnpm run visual-renderer:test
uv sync --frozen
uv run python -m unittest discover
python -m unittest discover .tekton -p '*_test.py'
cd infra/pulumi
bun install --frozen-lockfile
bun run build
bun run test
cd ../..
docker compose config --quiet
git diff --check
```

Run this sequence locally before pushing. The preview and release pipelines retain the pinned Gitleaks scan and delivery checks. They do not repeat the local quality suite. Each Faktory image receives a matching native renderer check before manifest publication.

The pipelines build `faktory-visual-renderer` only for AMD64 from the `visual-renderer-runtime` target. A native AMD64 task starts the packaged service under the production hardening policy. It renders a colored GLB and validates all seven PNG responses. Deployment receives the worker image by immutable digest.

## Releases

A release requires a committed remote `main` branch and an annotated, signed `vX.Y.Z` tag. The tag must point to a commit in `origin/main`. The version must equal both `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json`.

The release pipeline accepts only an exact stable semantic-version tag. It verifies the webhook SHA, tag signature, trusted signer, and `origin/main` ancestry. It publishes the Faktory AMD64 and ARM64 images. It promotes the immutable Faktory manifest digest to `vX.Y.Z`.

The same release gate publishes the AMD64-only `faktory-visual-renderer` image. It promotes that image's immutable digest to `vX.Y.Z`. Pulumi receives both digests for the `prod` stack. The pipeline does not publish a floating alias.

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

To exercise the same packaged renderer contract locally for the host architecture, build the runtime image and override its server entrypoint:

```bash
docker build --target runtime --tag faktory-renderer-verify:local .
docker run --rm --entrypoint /bin/sh faktory-renderer-verify:local -c '
set -eu
output_dir="$(mktemp -d /tmp/faktory-render-XXXXXX)"
trap '\''rm -rf "$output_dir"'\'' EXIT
/opt/faktory/env/bin/python -m renderer \
  /opt/faktory/renderer/examples/box.py \
  "$output_dir/model.glb" \
  "$output_dir/model.svg" \
  "$output_dir/model.json" \
  "$output_dir/isometric.svg" \
  "$output_dir/front.svg" \
  "$output_dir/back.svg" \
  "$output_dir/left.svg" \
  "$output_dir/right.svg" \
  "$output_dir/top.svg" \
  "$output_dir/bottom.svg"
/opt/faktory/env/bin/python -c '\''
import struct
import sys
from pathlib import Path

content = Path(sys.argv[1]).read_bytes()
if len(content) < 12:
    raise SystemExit("GLB header is truncated")
magic, version, declared_size = struct.unpack("<4sII", content[:12])
if magic != b"glTF" or version != 2 or declared_size != len(content):
    raise SystemExit("invalid GLB header")
'\'' "$output_dir/model.glb"
'
```

This local command validates only the architecture executed by the local Docker engine. It does not replace the pipeline's native AMD64 and ARM64 tasks or prove either remote task has run.

## Local Compose Integration

Compose initializes Garage and Faktory PostgreSQL from named volumes. It also starts one AMD64 visual renderer on internal service port `8081`. The worker has no host port.

Faktory uses `FAKTORY_VISUAL_RENDERER_URL=http://visual-renderer:8081` and `FAKTORY_VISUAL_RENDER_TIMEOUT_SECONDS=30`. Faktory waits for the worker readiness check before startup.

Faktory runs with exact `FAKTORY_AUTH_MODE=disabled`. The SPA, gRPC-web, artifact, and MCP routes require no cookie or bearer token. The Faktory image build selects the native Linux AMD64 or ARM64 renderer lock automatically.

All fixed Compose credentials are local-development-only. Open `http://localhost:8080` directly; no sign-in or MCP OAuth flow is used locally. The Faktory port is published only on `127.0.0.1`. Never expose this unauthenticated topology beyond loopback:

```bash
docker compose build
docker compose up --wait --wait-timeout 180
curl --fail http://localhost:8080/health
curl --fail http://localhost:8080/ready
docker compose down --volumes --remove-orphans
```

Use `docker compose logs faktory visual-renderer garage garage-init postgres` to inspect startup failures. The cleanup command removes all local application and storage state.

Tests and checks prove local source and declaration consistency. Compose exercises only explicit disabled authentication mode; it does not exercise or prove production Authentik, hosted MCP OAuth, Ceph, ingress, TLS, or deployed behavior. When `FAKTORY_AUTH_MODE` is absent, the server defaults to production authentication and fails closed if its required production configuration is missing or invalid.

## Visual Renderer Worker

The visual renderer supports only Linux AMD64. Its package checks run through the workspace commands in the validation sequence.

Build and verify the dedicated image from the repository root:

```bash
docker build --platform linux/amd64 --target visual-renderer-runtime --tag faktory-visual-renderer:local .
docker run --rm --detach --name faktory-visual-renderer-verify \
  --platform linux/amd64 \
  --user 65532:65532 \
  --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m \
  --cpus 1 \
  --memory 1g \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --security-opt seccomp=unconfined \
  --volume "$PWD/.tekton/verify-visual-renderer.mjs:/verify-visual-renderer.mjs:ro" \
  faktory-visual-renderer:local
docker exec faktory-visual-renderer-verify node /verify-visual-renderer.mjs
docker rm --force faktory-visual-renderer-verify
```

The verifier submits a colored GLB to `POST /v1/render`. It checks the seven canonical PNG images, dimensions, names, and visible color.

The worker runs as UID and GID `65532`. It has a read-only root filesystem, no capabilities, and no privilege escalation. Its memory-backed `/tmp` has a `256MiB` limit and no executable permission.

The outer worker container uses an unconfined seccomp profile because Chromium requires syscalls outside the default profile. Chromium keeps its own sandbox enabled. Faktory keeps the `RuntimeDefault` seccomp profile. Never disable the Chromium sandbox.

Preview and production expose the worker only through a ClusterIP Service. NetworkPolicy accepts port `8081` only from Faktory pods. The worker has no egress access, public route, ingress route, or authentication sidecar.
