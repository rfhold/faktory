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
pnpm run visual-renderer:build
pnpm run visual-renderer:typecheck
pnpm run visual-renderer:test
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

Pulumi declaration tests must cover two ObjectBucketClaims, generated credentials, separate storage classes, Barman policy, retention, hostname, data protection, telemetry, external ports, immutable images, and secret-free stack files. They must also cover one AMD64 visual worker, probes, port 8081, server URL wiring, and no public route. Security checks must cover UID/GID 65532, read-only root, no privilege escalation, dropped capabilities, disabled service-account tokens, 256 MiB memory-backed `/tmp`, worker-only `Unconfined` seccomp, server `RuntimeDefault`, server-only ingress, and empty worker egress.

Pipeline review must cover exact signed stable tags, trusted signer and `origin/main` checks, version equality, Gitleaks, both server architectures, packaged CadQuery execution, idempotent promotion, absence of `latest`, and immutable-digest deployment. A native AMD64 task must build and exercise the visual-renderer runtime image with Chromium sandbox enabled. Its packaged verifier must verify seven colored canonical PNGs and safe `422` rejection of a self-contained GLB that references an external resource. No test or declaration can claim native ARM64 visual-renderer support.

Native renderer checks use the uv environment from `pyproject.toml` and `uv.lock`. Each preview and release architecture task must run its just-built runtime image on a matching native node. It must invoke `/opt/faktory/env/bin/python -m renderer` with a project root, relative Python entrypoint, and empty library root. The project must produce an explicit two-output `Design`; the check must validate `outputs.json`, every primary and secondary worker artifact, both GLB frames, and the absence of server-owned shaded output. Manifest publication depends on both image checks. See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the pinned lock regeneration command and equivalent host-architecture project-root image check; the full local suite does not automatically execute remote AMD64 and ARM64 images.

The Compose lifecycle in [CONTRIBUTING.md](../../CONTRIBUTING.md) validates fresh-volume Garage initialization, service readiness, exact disabled authentication mode, and stdout-only local telemetry through `FAKTORY_DEPLOYMENT_ENVIRONMENT=local`. Faktory publishes `0.0.0.0:8080` and uses public base `http://172.16.1.40:8080`; this unauthenticated check requires a trusted private network and a host firewall restriction for TCP 8080. Garage stays loopback-only. The isolated AMD64 visual renderer has no published port. Compose bypasses OAuth redirect validation and does not validate production browser sessions, hosted MCP OAuth, TLS, OTLP export, Pyroscope, or an external Authentik instance.

## Required Protocol Coverage

Tests must cover unary methods, authentication failures, complete model fields, create/update view etag semantics, default-view clearing on deletion, validation of finite camera values and quaternions, and stable gRPC status mapping. Complete-model tests must include optional `current_successful_facts`, ordered `current_successful_outputs`, and `updated_at` in unary, snapshot, and change responses. They must prove that exactly one output summary is primary and that its facts equal `current_successful_facts`. Empty output summaries and absent primary facts must represent no successful geometry. Current local coverage does not constitute production authentication integration evidence.

Watch tests must prove that the first event is exactly one authoritative snapshot, later events are complete typed `model_changed` upserts, reconnect replaces prior client state, and no event implies unsupported model deletion.

## Required Storage and Render Coverage

Adapter and renderer tests must cover exact S3 keys, lowercase project digests, immutable canonical bundle writes, accepted result forms, invalid or absent `result`, GLB validation, bounded execution, restart reconciliation, and conditional view writes. Canonicalization fixtures must cover the exact JSON encoding and final LF, upload-order independence, strict ASCII path acceptance, non-ASCII path rejection, strict UTF-8 file content, BOM and newline normalization, deterministic byte ordering, duplicate exact paths, every path rejection, 256-file and 1,048,576-byte boundaries, explicit entrypoint validation, and revision changes for every identity-bearing field.

Exact-key coverage must include `project.json`, `outputs.json`, and every `outputs/{output_id}` artifact path from [`../architecture/storage-rendering.md`](../architecture/storage-rendering.md). Tests must prove that only the primary path has canonical shaded and named-view objects. Legacy fixtures must retain fixed-key selection without permitting new revisions to write fixed keys.

Project MCP tests must prove the hard removal of singular `source` schemas. Create-schema tests must require only `model_id`, `name`, `files`, and `entrypoint`, with empty defaults for optional `requirements` and `hints`. Get-schema and result tests must accept only `model_id` and return complete metadata plus all desired-project file contents, requirements, locks, and generated `AGENTS.md`. Edit tests must cover one ordered operations array with add, exact patch, delete, rename, entrypoint, dependency, and hint operations; whole-edit revision guards; operation rollback on validation failure; protected AGENTS sections; generated index and dependency guidance; and name-only edits. Exact-patch tests must reject empty, absent, ambiguous, and no-op matches at the step where they occur.

Workspace schema tests must reject unknown top-level and nested fields. Open tests must cover desired-revision identity, metadata, entrypoint, requirements, locks, full generated `AGENTS.md`, content-hash indexes, and omitted caller-file bodies. Read tests must cover desired and exact revisions, one-based slicing, defaults, maximums, empty and final-line behavior, returned revision identity, and invalid offsets. Glob tests must cover ASCII-only syntax, literal separators, component-local `*`, `?`, and character classes, recursive `**`, 1,024-byte patterns, canonical order, a 1,000-path cap, and truncation. Grep tests must cover Rust regex rejection, recursive include globs, canonical result order, one-based lines, default and maximum result limits, 2,000-byte UTF-8-safe line truncation, and result truncation. Multibyte tests must prove that runtime validation enforces the 1,024-byte regex limit without a JSON Schema `maxLength`.

Apply-patch tests must cover the 1 MiB UTF-8 runtime bound without a JSON Schema `maxLength`. They must cover exact envelope markers, optional envelope-final LF, every section type, rename-only updates, and ordered hunks. They must cover file-final LF representation, protected `AGENTS.md`, exact-once matches, path conflicts, stale revisions, final-project no-ops, and full rollback. A successful patch must create one revision, report ordered changed paths, and schedule exactly one render. Failed patches must schedule none.

Shared-library tests must cover stable exact SemVer parsing, monotonically increasing immutable publication, namespaced package layout, guidance and docs, release hashing, version conflicts, and prerelease or build rejection. Library workspace tests must cover exact release selection, guidance and content-hash indexes, omitted file bodies, bounded line reads, and strict unknown-field rejection. Server tests must reject forbidden static cross-library statement forms before publication. Python-worker tests must parse every library file and reject syntax-aware multiline, aliased, and relative cross-library imports before execution. Tests must not imply protection against dynamic malicious imports or reject imports available from the fixed renderer runtime and Python standard library. Requirement tests must cover the lack of external dependency declaration, installation, or resolution, exact same-major ranges, highest-compatible initial locks, exact release hashes, and explicit major adoption. Rollout tests must prove durable publication intent, same-major minor and patch adoption, no automatic major adoption, deterministic AGENTS regeneration, per-model idempotence, restart resume, concurrent model re-evaluation, failed-render advancement, and ordinary last-good preservation. Retry tests must prove that no dependency is re-resolved.

Migration tests must run before repository validation and reconciliation and must cover ordered one-module registration, unknown or newer ledgers, per-model checkpoints, restart at every write boundary, byte-identical collision acceptance, conflicting-copy rejection, and completion only after verification. Legacy project fixtures must prove deterministic `source.py` project creation, empty requirements and hints, artifact and named-view-cache copying, revision remapping, orphan inventory, recovery backups, desired/current divergence, failed and pending states, exact last-good preservation, no render enqueue, no source leakage, and no deletion of legacy objects. Rollback tests must enforce the forward-only boundary after metadata cutover.

Multipart compatibility tests must prove that a revision without `outputs.json` needs no migration or backfill. It must expose one synthetic `primary` output with role `OUTPUT_ROLE_ASSEMBLY`, primary facts, fixed-key artifacts, and primary named-view behavior. Every other output selection must fail closed. Tests must prove that startup does not rewrite or delete the legacy revision.

Renderer tests must validate the `faktory_design.v1` API and direct legacy CadQuery values. Explicit-design tests must cover 1 and 64 outputs, declaration order, all three roles, each accepted geometry type, and exactly one primary. Boundary and negative tests must cover 0 and 65 outputs, duplicate IDs, the 64-byte ID limit, every ID syntax rejection, unknown fields, invalid roles, unsupported geometry, and zero or multiple primary outputs. Legacy normalization must map `Assembly` to primary `assembly` and `Workplane` or `Shape` to primary `part`, always with ID `primary`.

Renderer tests must validate each output's total compound and assembly component volume in cubic millimetres. Overlap fixtures must prove that components count independently without a boolean union. Tests must validate source-coordinate axis-aligned x/y/z dimensions in millimetres before glTF export.

Renderer tests must validate each output's non-empty, well-formed preview SVG and GLB, fixed projection names, and seven technical projections. Tests must enforce the exact and adjacent per-GLB and aggregate worker-bundle boundaries from [`../architecture/design-bundles.md`](../architecture/design-bundles.md) without oversized fixtures. Python coverage must accept 64 outputs with deterministic manifest order and reject aggregate overflow before atomic publication; Rust must independently reject aggregate overflow before rasterization or complete result construction. Technical orientation fixtures must verify each exact camera direction and screen-right vector through the explicit `gp_Ax2` basis. Rust rasterization tests must validate signatures, exact 640x480 dimensions, deterministic bytes, opaque white background, the 512 KiB cap, and complete output construction. Missing, oversized, or malformed output and invalid facts must fail the complete design bundle before metadata advancement.

Shared camera tests must cover source-to-Three mapping, all canonical direction and screen-right pairs, and derived screen-up. They must cover perspective framing and the 82 percent fill rule. Browser tests must exercise actual Three.js renders, not only camera math. They must verify preserved GLB colors and materials, Soft light values, dark background, sRGB, no tone mapping, exposure, antialiasing, disabled shadows, opaque 640x480 output, and every canonical orientation. Cross-platform byte equality is not required.

View-camera tests must cover perspective and orthographic saved views. They must verify that orthographic scale equals `(top-bottom)/zoom`, application resets zoom to 1, and resize preserves effective span. Projection-toggle tests must preserve footprint in both directions and prevent round-trip drift. Legacy lost zoom remains unrecoverable and requires an explicit compatibility test.

Visual RPC tests must reject a wrong method, content type, header count, base64 encoding, schema, recipe, camera, GLB, image name, order, MIME type, dimensions, alpha, and size. Validator tests must cover each exact and adjacent 64 MiB request, 16 KiB header, 2 MiB image, and 24 MiB response bound without requiring oversized fixtures. They must cover concurrency one, queue capacity eight, saturation, the complete fixed 25-second deadline, stable `504 render_timeout` mapping, browser or render rejection recovery, and malformed responses. Deterministic cancellation tests must prove that pending disconnects immediately free queue capacity and active disconnects close browser work, dispose harness data, release the slot, and admit the next request. Tests must also prove that late completion cannot write a second response or create an unhandled rejection. Route interception tests and the packaged Chromium verifier must prove that external model dependencies and outbound browser requests fail with safe bounded diagnostics.

Replacement tests must prove atomic current-success advancement only after the manifest, every declared output artifact, primary shaded set, summaries, and facts validate. Injected failure at every output and write boundary must preserve the complete prior bundle. No model record can expose mixed revisions, mixed output sets, or facts from another manifest. Pending and active replacements must preserve the same last-good data. A first-render failure must leave the manifest, summaries, artifacts, and facts unavailable while it exposes safe failure state. Tests must prove that no partial output success exists.

Manifest tests must cover exact `faktory-outputs-v1` bytes, final LF, declared order, finite non-negative facts, one primary, unknown-member rejection, and immutable collision behavior. Access tests must enforce current-success gates, immutable conflicts, recipe-version invalidation, legacy shaded absence, synthetic legacy output interpretation, and last-good behavior.

Named-view cache tests must key multipart identity by project revision, primary output ID, view ID, etag, and recipe. They must cover hit, miss, in-flight deduplication, corrupt bytes, worker failure, and immutable conflicts. Revision, primary-output, and etag races before storage or return must produce conflict. Tests must prove that stale cache objects never become selectable and that saved-view inspection can use a legacy primary GLB. They must reject `output_id` on `view.inspect` and expose no non-primary named-view path.

MCP tests must cover `model.inspect` with omitted and explicit `output_id`, default technical style, and explicit shaded style. Omission must select the primary. Technical requests must select every output. Shaded requests must accept only the primary and reject explicit non-primary IDs as invalid argument. Each success returns one validated semantic PNG block plus output ID, role, and primary metadata without image bytes. Tests must cover fresh and stale revisions, output absence, synthetic legacy selection, warning text, recipe metadata, no successful render, missing legacy images, and corrupt bytes.

`view.inspect` tests must cover exact input, primary output metadata, saved-camera conversion, one shaded image, cache behavior, stale results, and identity conflicts. Streamable HTTP tests must call both tools through the actual MCP router and preserve each semantic image block. Error tests must verify safe invalid-argument, not-found, conflict, unavailable, retry-enabled, and invalid-state mappings.

Timestamp tests must prove that accepted model creates, project edits, compatible-library rollout revisions, and name edits change `updated_at`. Render-state transitions and view mutations must preserve it.

## Required Access Coverage

Disabled-mode tests must cover unauthenticated SPA, gRPC-web, HTTP artifacts, and MCP routing, plus exact mode selection and production-by-default behavior. Production browser integration tests must cover Authentik login, server-owned session expiry, gRPC-web calls, authenticated HTTP artifacts, and shared view edits by distinct authenticated users. Production MCP integration tests must cover hosted OAuth discovery, authorization-code security, resource-bound Faktory tokens, exact desired-revision project-file retrieval, model creation and project editing, library publication and retrieval by any authenticated MCP principal, and rejection of browser or Authentik tokens at MCP routes.

Artifact route tests must cover production session enforcement and disabled-mode access for primary aliases and output-aware paths. They must cover every declared output, current-successful revision gates, last-good access, unknown outputs, synthetic legacy `primary`, and `404` responses before first success. Alias tests must prove that primary bytes and validators equal the output-aware responses. Preview tests must verify `image/svg+xml`, `Cache-Control: private, no-cache`, output-aware entity tags, and conditional requests. GLB tests retain equivalent gate and cache coverage plus range coverage.

Catalog tests must prove one row per model and lazy primary-preview fetches. They must verify semantic labels for primary volume and x/y/z dimensions. Models without successful geometry must show unavailable facts. Accessibility tests must cover useful preview alternatives, keyboard access, and status announcements. Responsive tests must cover narrow and wide viewports without clipped facts or controls.

Negative tests must prove that credentials, tokens, project or library source, AGENTS content, library guidance or docs, GLB, SVG, PNG bytes, object-store internals, and raw renderer output do not leak through logs, signals, errors, protobuf, or browser responses. Exact source-bearing content appears only in authorized MCP project and library tool results. Open results must omit indexed bodies. Bounded tools must not exceed their content and result caps. One PNG appears only in authorized `model.inspect` or `view.inspect` results. Worker, rollout, and migration logs and errors must follow the same boundary. Browser tests enforce the Faro boundary in [`../architecture/observability.md`](../architecture/observability.md).

Docker hardening tests must inspect both runtime images. Visual-worker checks must prove that the image excludes the SPA, Rust server, and CadQuery environment. Chromium launch tests must prove `chromiumSandbox: true` and absence of `--no-sandbox`. Security declaration tests must retain the narrow worker-only seccomp exception and all compensating controls.

## Evidence Boundary

Source inspection, focused unit tests, the full local suite, mocked Pulumi declaration tests, Compose integration, pipeline execution, and deployed verification are separate evidence layers. Passing one layer must not be described as evidence for another. Repository checks and declarations prove neither pipeline execution nor live collector ingestion, backup completion, restoration, authentication, storage, routing, rendering, model mutation, preview operation, or production operation. Every live check requires explicit target authority.
