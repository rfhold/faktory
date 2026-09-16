# Storage and Rendering

The current project-key layout, project mutation language, and ordered legacy migration exist in the repository. The multipart layout and advancement rules below define approved behavior under implementation. This repository does not prove a preview or production deployment. [`design-bundles.md`](design-bundles.md) owns output semantics. [`../operations/object-store-migrations.md`](../operations/object-store-migrations.md) defines conversion before repository validation or render reconciliation.

## Object Layout

The runtime storage adapter uses one S3-compatible artifact bucket with these exact keys. Local Compose supplies Garage 2.3 as that object store:

```text
models/{model_id}/model.json
models/{model_id}/revisions/{project_sha256}/project.json
models/{model_id}/revisions/{project_sha256}/outputs.json
models/{model_id}/revisions/{project_sha256}/outputs/{output_id}/model.glb
models/{model_id}/revisions/{project_sha256}/outputs/{output_id}/preview.svg
models/{model_id}/revisions/{project_sha256}/outputs/{output_id}/projections/{projection}.png
models/{model_id}/revisions/{project_sha256}/outputs/{primary_output_id}/renders/three-v2/canonical/{projection}.png
models/{model_id}/revisions/{project_sha256}/outputs/{primary_output_id}/renders/three-v2/views/{view_id}/{etag}.png
models/{model_id}/views/{view_id}.json
libraries/{library_name}/releases/{version}/release.json
libraries/{library_name}/index.json
system/library-rollouts/{library_name}/{version}/{release_sha256}.json
system/migrations/{migration_id}.json
```

`model_id` is a caller-supplied, immutable identifier. It contains at most 64 ASCII bytes and matches `^[a-z0-9]+(-[a-z0-9]+)*$`. `output_id` uses the same syntax and byte limit. View IDs and etags are server-issued opaque UUIDs. `project_sha256` is the lowercase hexadecimal SHA-256 of the canonical project bundle defined in [`model-projects-libraries.md`](model-projects-libraries.md). Project bundles, output manifests, artifacts, and release objects are immutable.

Each new successful revision owns `outputs.json` and one complete output directory per declared output. Every output directory contains one GLB, one preview, and seven technical PNGs. Each GLB has a 64 MiB cap. Only the primary directory contains seven canonical shaded PNGs and named-view cache entries. `projection` accepts only `isometric`, `front`, `back`, `left`, `right`, `top`, and `bottom`; arbitrary object-key input is forbidden. Recipe-versioned paths invalidate caches when visual semantics change. Named-view keys bind the project revision, primary output ID, view ID, etag, and recipe. Migration subkeys and recovery backups are defined in [`../operations/object-store-migrations.md`](../operations/object-store-migrations.md).

Revisions without `outputs.json` retain the legacy fixed keys `model.glb`, `preview.svg`, `projections/{projection}.png`, `renders/three-v2/canonical/{projection}.png`, and `renders/three-v2/views/{view_id}/{etag}.png`. New multipart revisions never write those keys. The repository interprets them through the synthetic legacy primary output defined in [`design-bundles.md`](design-bundles.md).

Model and view creation and all immutable writes retain the object store's atomic `If-None-Match: *` request. A repeated immutable write accepts identical bytes and rejects conflicting bytes. For matched mutable `model.json` and view JSON changes and deletes, the process serializes mutations, reads the current object ETag, rejects a mismatch, and then sends an unconditional mutation. This process-local optimistic concurrency requires one server replica with a `Recreate` strategy.

`model.json` is the authority for display name, desired project revision, current successful project revision, render state, safe render error, default view ID, optional current-successful output summaries, the primary facts alias, and `updated_at`. The unchanged protobuf fields retain `source_revision` in their names as a wire-compatibility label. Output summaries and facts belong to the recorded current successful revision and match its immutable manifest. Each view object contains the protobuf-equivalent camera fields and etag metadata. JSON schema details must be fixed with the storage adapter; implementations must not infer an alternative key layout.

`updated_at` records the latest accepted model create, project edit, compatible-library rollout, or name edit. Render-state transitions and view mutations do not change it.

## Declared Bucket Ownership

[`infra/pulumi/index.ts`](../../infra/pulumi/index.ts) declares separate `faktory-artifacts` and `faktory-backups` `ObjectBucketClaim` resources in each stack namespace. The object-bucket provisioner, not Pulumi configuration or a human-supplied credential, generates each bucket name, ConfigMap, and Secret. The server consumes `BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY` from the artifact claim. CloudNativePG consumes the backup claim's bucket name and credential Secret directly. Artifact objects and database backup objects therefore have separate credentials and lifecycles.

The production stack sets `protectData=true`, which applies Pulumi protection to both claims and the PostgreSQL cluster. Preview sets it to false. Protection prevents an ordinary Pulumi delete or replacement of those protected resources; it is not a backup, does not cover every stack resource, and does not override the object-bucket storage class's reclaim policy. No declaration proves that either claim or its generated bucket exists.

## Project Contract

[`model-projects-libraries.md`](model-projects-libraries.md) is authoritative for canonical files, managed `AGENTS.md`, dependencies, exact locks, and MCP file operations. Creation succeeds only when the model ID is absent; a current model conflicts. After the explicit Python entrypoint executes with the locked direct libraries available, its top-level `result` must satisfy [`design-bundles.md`](design-bundles.md). Imports and other top-level Python statements remain allowed under the trusted-source MVP assumption. CQGI parameters are excluded.

A name-only edit updates metadata without a project revision, render work, or render-state change. MCP project and library tools are the only source-bearing interfaces. MCP `model.list`, inspect results, protobuf responses, and browser responses remain source-free metadata or artifacts; the browser has no project route or editor.

## Replacement Rendering

1. Validate and canonicalize the edited project, resolve dependencies only when creation or an explicit dependency edit requires it, compute `project_sha256`, and persist immutable `project.json` before marking that revision desired.
2. Set the desired revision and render state to `PENDING`, then `RENDERING` when work starts.
3. Preserve the complete current-successful design bundle while the replacement renders.
4. Normalize `result` and validate 1 through 64 unique outputs, their roles, declaration order, and exactly one primary selection.
5. For every output, compute facts and source-coordinate dimensions before glTF export. Preserve the current component-sum volume semantics.
6. For every output, produce and validate one temporary GLB, one preview SVG, and seven technical SVGs. Enforce the per-file and aggregate worker-bundle capacity contract in [`design-bundles.md`](design-bundles.md) before publishing or consuming the complete worker output.
7. Rasterize every technical SVG onto an opaque white background inside the bounded Rust render worker with minimal-feature `resvg` 0.45.1. Each PNG must be 640x480 and at most 512 KiB.
8. Send only the primary GLB to the isolated visual renderer. It returns all seven canonical shaded PNGs for recipe `three-v2`.
9. Construct and validate the immutable `outputs.json` manifest. Store it and every required output artifact only after the complete bundle passes validation.
10. After every immutable write succeeds, atomically advance the current successful revision and its output summaries. Replace the primary facts alias, clear the safe error, and set `READY`.
11. On any failure, preserve the prior current successful revision and its complete bundle. Set the desired revision state to `FAILED` and record only a safe bounded error.

Canonical shaded objects are never overwritten. The ordered legacy migration copies and remaps existing objects without deleting them. Existing `three-v1` objects remain legacy and unselected after `three-v2` becomes active. An existing revision needs a successful rerender before canonical v2 inspection can select its seven `three-v2` artifacts; until then, canonical shaded inspection returns not found while technical images retain current behavior. Saved-view inspection can use the legacy synthetic primary GLB and populate its fixed-key `three-v2` cache through the existing on-demand render path.

The one server replica uses an in-process bounded rendering queue. On restart, models left `PENDING` are queued without rewriting `model.json`; interrupted `RENDERING` models are reset to `PENDING` before being queued. Reconciliation is serialized with repository mutations. Queue limits, renderer timeout, output limits, and subprocess behavior are explicit runtime configuration.

## Projection Semantics

The CadQuery HLR adapter passes an explicit OpenCascade `gp_Ax2` direction and screen-right basis. This fixes both view direction and roll. Stored images remain immutable, so corrected technical orientation appears only after a later successful project revision.

| Projection | Source camera direction | Source screen-right | Three camera direction | Three screen-right |
| --- | --- | --- | --- | --- |
| `isometric` | `(1,-1,1)` | `(1,1,0)` | `(1,1,1)` | `(1,0,-1)` |
| `front` | `(0,-1,0)` | `(1,0,0)` | `(0,0,1)` | `(1,0,0)` |
| `back` | `(0,1,0)` | `(-1,0,0)` | `(0,0,-1)` | `(-1,0,0)` |
| `left` | `(-1,0,0)` | `(0,-1,0)` | `(-1,0,0)` | `(0,0,1)` |
| `right` | `(1,0,0)` | `(0,1,0)` | `(1,0,0)` | `(0,0,-1)` |
| `top` | `(0,0,1)` | `(1,0,0)` | `(0,1,0)` | `(1,0,0)` |
| `bottom` | `(0,0,-1)` | `(1,0,0)` | `(0,-1,0)` | `(1,0,0)` |

## Shaded Recipe

`web/src/viewer/threeRecipe.ts` defines the shared `three-v2` browser and worker semantics with Three.js 0.180.0. Source coordinates map to Three coordinates as `(x,y,z) -> (x,z,-y)`. The table records the unnormalized vectors before and after this transform. Canonical cameras normalize both Three vectors. They derive screen-up as `cross(direction, screen-right)`, use perspective projection, and frame all bounds corners at 82 percent fill.

The worker renders opaque 640x480 PNGs at device pixel ratio 1. It uses antialiasing, sRGB output, `NoToneMapping`, exposure `1`, disabled shadows, and background `#11150f`. The Soft setup uses ambient `0xdde5d7` at `1.4`, hemisphere `0xf4f7ed` over `0x68705f` at `2.2`, and three directional lights targeted at the origin. Two directional lights use `0xe8eee2` at `0.9`, from `(4,5,6)` and `(-4,3,-6)`. A restrained upward underside fill uses `0xc7d2c2` at `0.55` from `(0,-6,2)` in Three.js Y-up coordinates, making bottom-facing surfaces distinct from the dark background without changing the Studio preset. The recipe preserves GLB materials. The canonical perspective camera uses a 42-degree vertical field of view and near/far planes `0.01` and `100000`.

The SPA uses the same recipe and defaults to Soft lights. Its optional Studio helpers remain hidden by default. Interactive device pixel ratio has a cap of 2. Shared scene semantics require visual equivalence, not cross-platform PNG byte identity.

## Named-View Render Cache

`view.inspect` first requires a current successful revision and an exact current named view. A cache miss loads that revision's primary GLB and requests one bounded worker render. The server deduplicates concurrent work by revision, primary output ID, view ID, and view etag. Named views never select a non-primary output. The server never executes source or launches Chromium in the application process.

Before an immutable cache write, the repository rechecks the current successful revision and view etag under its mutation lock. It repeats the identity check before return. A changed revision or etag produces a conflict and prevents mismatched output. Obsolete immutable cache objects can remain unreachable; no request can select them through a current identity.

## Security Boundary

MVP project and library source is trusted operational input but remains capable of arbitrary Python behavior. The renderer is not a hostile-code sandbox. Production source access for untrusted principals is forbidden until a separately reviewed isolation design exists.
