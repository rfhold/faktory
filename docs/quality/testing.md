# Testing

## Local Checks

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
(cd infra/pulumi && bun install --frozen-lockfile && bun run build && bun run test)
docker compose config --quiet
git diff --check
```

Protocol generation must be reproducible with `pnpm run generate:proto`, and a clean regeneration must produce no diff. Relative documentation links must resolve locally. The Pulumi tests inspect declarations with mocks and do not contact Kubernetes, Authentik, Ceph, or PostgreSQL.

## Focused Telemetry Checks

Use focused checks while changing telemetry, then run the full sequence above before acceptance:

```bash
cargo +1.96.0 test --locked -p faktory-server observability
cargo +1.96.0 test --locked -p faktory-server profiling
cargo +1.96.0 test --locked -p faktory-server telemetry
cargo +1.96.0 test --locked -p faktory-server dropped_request_is_finalized_as_cancelled
pnpm --filter @rfhold/faktory-web exec vitest run src/telemetry.test.ts
cd infra/pulumi
bun run build
bun run test
cd ../..
```

Backend tests must cover endpoint pairing, fixed resource identity, target and level allowlists, credential-free profiler origins, stable low-cardinality profile tags, bounded shutdown, HTTP method and route classification, probe exclusion, status and outcome mapping, and cancellation counter balancing. Negative tests must show that raw paths, queries, IDs, headers, bodies, source, credentials, tokens, object data, renderer output, and arbitrary error text cannot enter server signals.

Frontend tests must cover the production-build-only gate, including telemetry enabled in production-mode preview images; exact Faro `2.11.0` identity, endpoint, environment, and version behavior; one-shot fail-open initialization; explicit application view normalization; and bounded custom errors. Tests must also preserve the accepted privacy behavior: standard Faro instrumentation remains enabled without a global `beforeSend` sanitizer or URL ignore list, and no session replay integration is added. This acceptance does not weaken the stricter server leakage policy.

Pulumi declaration tests must cover two ObjectBucketClaims, generated artifact and backup configuration and credential wiring, separate storage classes, Barman destination and gzip settings, immediate daily 02:00 UTC scheduled backup, stack-specific 14-day and 30-day retention, production hostname and data protection, telemetry endpoints and Kubernetes identity, external ports, immutable image policy, and secret-free stack files. Pipeline review must cover exact signed stable tags, trusted signer and `origin/main` checks, version equality, Gitleaks, both architectures, per-architecture packaged renderer execution before manifest publication, idempotent stable promotion, absence of `latest`, and immutable-digest production deployment.

Native renderer checks use the uv environment from `pyproject.toml` and `uv.lock`. Each preview and release architecture task must run its just-built runtime image on a matching native node, execute `/opt/faktory/env/bin/python -m renderer` against packaged `renderer/examples/box.py`, and reject a generated GLB unless its magic is `glTF`, its little-endian version is 2, and its declared byte size equals the file size. Manifest publication depends on both image checks. See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the pinned lock regeneration command and a host-architecture local image check; the full local suite does not automatically execute remote AMD64 and ARM64 images.

The Compose lifecycle in [CONTRIBUTING.md](../../CONTRIBUTING.md) validates fresh-volume Garage initialization, service readiness, explicit disabled authentication mode on loopback, and stdout-only local telemetry through `FAKTORY_DEPLOYMENT_ENVIRONMENT=local`. Compose does not exercise or validate production browser sessions, hosted MCP OAuth, TLS, OTLP export, Pyroscope, or an external Authentik instance.

## Required Protocol Coverage

Tests must cover unary methods, authentication failures, complete model fields, create/update view etag semantics, default-view clearing on deletion, validation of finite camera values and quaternions, and stable gRPC status mapping. Complete-model tests must include optional `current_successful_facts` and `updated_at` in unary, snapshot, and change responses. Current local coverage does not constitute production authentication integration evidence.

Watch tests must prove that the first event is exactly one authoritative snapshot, later events are complete typed `model_changed` upserts, reconnect replaces prior client state, and no event implies unsupported model deletion.

## Required Storage and Render Coverage

Adapter and renderer tests cover exact S3 keys, lowercase source digests, UTF-8 rejection, immutable revision writes, accepted CadQuery result types, invalid or absent `result`, GLB validation, bounded execution, restart reconciliation, and conditional view writes. Exact-key coverage includes `models/{model_id}/revisions/{source_sha256}/preview.svg` and every closed `models/{model_id}/revisions/{source_sha256}/projections/{projection}.png` key.

Renderer tests must validate total compound and assembly component volume in cubic millimetres. Overlap fixtures must prove that components count independently without a boolean union. Tests must validate source-coordinate axis-aligned x/y/z dimensions in millimetres before glTF export.

Renderer tests must validate non-empty, well-formed SVG output and GLB output, fixed projection names and vectors, deterministic generation, and all seven projection outputs. Rust rasterization tests must validate PNG signatures, exact 640x480 dimensions, deterministic bytes, an opaque white untouched background pixel, the 512 KiB per-image cap, and complete output construction. Missing, oversized, or malformed output and invalid facts must fail the render before metadata advancement.

Replacement tests must prove atomic current-success advancement only after every immutable artifact write and valid facts. No model record can expose artifacts or facts from mixed revisions. Every earlier failure must preserve the prior revision, GLB, preview, projections, and facts. Pending and rendering replacements must preserve the same last-good data. A first-render failure must leave all artifacts and facts unavailable while it exposes safe failure state. Projection access must enforce the current-success revision gate, immutable key behavior, legacy-image absence, and retained last-good behavior.

MCP tests cover the exact `model.inspect` projection schema, one validated semantic PNG image block, metadata without image bytes, fresh and stale revision semantics, explicit stale warning text, and safe errors for no successful render, a missing legacy image, or corrupt stored PNG bytes. Streamable HTTP coverage exercises a stateless `tools/call` through the actual Faktory MCP router and proves that the semantic image block survives transport.

Timestamp tests must prove that accepted model creates, source edits, and name edits change `updated_at`. Render-state transitions and view mutations must preserve it.

## Required Access Coverage

Disabled-mode tests must cover unauthenticated SPA, gRPC-web, HTTP artifacts, and MCP routing, plus exact mode selection and production-by-default behavior. Production browser integration tests must cover Authentik login, server-owned session expiry, gRPC-web calls, authenticated HTTP artifacts, and shared view edits by distinct authenticated users. Production MCP integration tests must cover hosted OAuth discovery, authorization-code security, resource-bound Faktory tokens, exact desired-revision source retrieval, model creation and source editing by any authenticated MCP principal, and rejection of browser or Authentik tokens at MCP routes.

Preview route tests must cover production session enforcement and disabled-mode access. They must cover the current-successful revision gate, last-good access, and `404` responses before first success. They must verify `image/svg+xml`, `Cache-Control: private, no-cache`, revision-derived entity tags, and conditional requests. GLB route tests retain equivalent gate and cache coverage plus range coverage.

Catalog tests must prove one row per model and lazy preview fetches. They must verify semantic labels for volume and x/y/z dimensions. Models without successful geometry must show unavailable facts. Accessibility tests must cover useful preview alternatives, keyboard access, and status announcements. Responsive tests must cover narrow and wide viewports without clipped facts or controls.

Negative tests must prove that credentials, tokens, Python source, GLB, SVG, or PNG bytes, object-store internals, and raw renderer output do not leak through server logs, traces, metrics, errors, protobuf responses, or browser responses. Exact source is intentionally allowed only in authorized MCP `model.get` results, and one projection PNG is intentionally allowed only in authorized MCP `model.inspect` results. Browser testing instead enforces the explicit Faro boundary in [`../architecture/observability.md`](../architecture/observability.md): Faktory-owned values are bounded, while accepted standard instrumentation can collect URLs, console, exception, browser, resource, and session metadata.

## Evidence Boundary

Source inspection, focused unit tests, the full local suite, mocked Pulumi declaration tests, Compose integration, pipeline execution, and deployed verification are separate evidence layers. Passing one layer must not be described as evidence for another. Repository checks and declarations prove neither pipeline execution nor live collector ingestion, backup completion, restoration, authentication, storage, routing, rendering, model mutation, preview operation, or production operation. Every live check requires explicit target authority.
