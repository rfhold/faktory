# Storage and Rendering

## Object Layout

The runtime storage adapter uses one S3-compatible artifact bucket with these exact keys. Local Compose supplies Garage 2.3 as that object store:

```text
models/{model_id}/model.json
models/{model_id}/revisions/{source_sha256}/source.py
models/{model_id}/revisions/{source_sha256}/model.glb
models/{model_id}/revisions/{source_sha256}/preview.svg
models/{model_id}/revisions/{source_sha256}/projections/{projection}.png
models/{model_id}/revisions/{source_sha256}/renders/three-v2/canonical/{projection}.png
models/{model_id}/revisions/{source_sha256}/renders/three-v2/views/{view_id}/{etag}.png
models/{model_id}/views/{view_id}.json
```

`model_id` is a caller-supplied, immutable identifier. It contains at most 64 ASCII bytes and matches `^[a-z0-9]+(-[a-z0-9]+)*$`. View IDs and etags are server-issued opaque UUIDs. `source_sha256` is the lowercase hexadecimal SHA-256 of the exact accepted source bytes. Revision source objects are immutable. Each new successful revision owns immutable `model.glb`, `preview.svg`, seven technical PNGs, and seven canonical shaded PNGs. `projection` accepts only `isometric`, `front`, `back`, `left`, `right`, `top`, and `bottom`; arbitrary object-key input is forbidden. Recipe-versioned paths invalidate caches when visual semantics change. Named-view keys bind the source revision, view ID, etag, and recipe.

Model and view creation and all immutable writes retain the object store's atomic `If-None-Match: *` request. A repeated immutable write accepts identical bytes and rejects conflicting bytes. For matched mutable `model.json` and view JSON changes and deletes, the process serializes mutations, reads the current object ETag, rejects a mismatch, and then sends an unconditional mutation. This process-local optimistic concurrency requires one server replica with a `Recreate` strategy.

`model.json` is the authority for display name, desired source revision, current successful source revision, render state, safe render error, default view ID, optional current-successful facts, and `updated_at`. Facts belong to the recorded current successful revision. They contain total volume in cubic millimetres and source-coordinate axis-aligned x/y/z dimensions in millimetres. Each view object contains the protobuf-equivalent camera fields and etag metadata. JSON schema details must be fixed with the storage adapter; implementations must not infer an alternative key layout.

`updated_at` records the latest accepted model create, source edit, or name edit. Render-state transitions and view mutations do not change it.

## Declared Bucket Ownership

[`infra/pulumi/index.ts`](../../infra/pulumi/index.ts) declares separate `faktory-artifacts` and `faktory-backups` `ObjectBucketClaim` resources in each stack namespace. The object-bucket provisioner, not Pulumi configuration or a human-supplied credential, generates each bucket name, ConfigMap, and Secret. The server consumes `BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY` from the artifact claim. CloudNativePG consumes the backup claim's bucket name and credential Secret directly. Artifact objects and database backup objects therefore have separate credentials and lifecycles.

The production stack sets `protectData=true`, which applies Pulumi protection to both claims and the PostgreSQL cluster. Preview sets it to false. Protection prevents an ordinary Pulumi delete or replacement of those protected resources; it is not a backup, does not cover every stack resource, and does not override the object-bucket storage class's reclaim policy. No declaration proves that either claim or its generated bucket exists.

## Source Contract

`model.create` requires `model_id`, `name`, and `source`. Creation succeeds only when the model ID is absent; a current model conflicts. The source is exactly one non-empty UTF-8 Python file. After execution, its top-level `result` must be a CadQuery `Workplane`, `Shape`, or `Assembly`. Imports and other top-level Python statements are allowed under the trusted-source MVP assumption. CQGI parameters and multi-file projects are excluded.

`model.edit` requires `model_id`, `expected_revision`, and at least one of `name` or a non-empty `patches` array. Each patch contains `old` and `new`. The server applies patches sequentially to the desired source. Each non-empty `old` value must match exactly once at its step. The server rejects a stale revision, an empty `old`, a missing or ambiguous match, and an overall no-op source edit.

A name-only edit updates metadata without a source revision, render work, or render-state change. MCP `model.get` loads model metadata first, then returns the immutable UTF-8 source selected by that record's desired revision. MCP `model.list`, protobuf responses, and browser responses remain metadata-only. Source retrieval, creation, and edits are MCP-only; the browser has no source route or editor.

## Replacement Rendering

1. Validate the edited UTF-8 source, compute `source_sha256`, and persist immutable `source.py` before marking that revision desired.
2. Set the desired revision and render state to `PENDING`, then `RENDERING` when work starts.
3. Preserve the current successful revision, GLB, preview, images, and facts while the replacement renders.
4. Compute facts from the CadQuery result before glTF export. Sum compound or assembly component volumes without a boolean union, so overlaps can count independently.
5. Compute source-coordinate axis-aligned dimensions before glTF export. Produce and validate temporary GLB, preview SVG, and seven technical SVG outputs.
6. Rasterize technical SVGs onto an opaque white background inside the bounded Rust render worker with minimal-feature `resvg` 0.45.1. Each PNG must be 640x480 and at most 512 KiB.
7. Send the GLB once to the isolated visual renderer. It returns all seven canonical shaded PNGs for recipe `three-v2`.
8. Store immutable `model.glb`, `preview.svg`, all technical PNGs, and all shaded PNGs after all outputs and facts pass validation.
9. After every immutable write succeeds, atomically advance the current successful revision, replace its facts, clear the safe error, and set `READY`.
10. On any failure, preserve the prior current successful revision, artifacts, and facts. Set the desired revision state to `FAILED` and record only a safe bounded error.

Canonical shaded objects are never migrated or overwritten. Existing `three-v1` objects remain legacy and unselected after `three-v2` becomes active. An existing revision needs a successful rerender before canonical v2 inspection can select its seven `three-v2` artifacts; until then, canonical shaded inspection returns not found while technical images retain current behavior. Saved-view inspection can use the current-successful GLB and populate its `three-v2` cache through the existing on-demand render path.

The one server replica uses an in-process bounded rendering queue. On restart, models left `PENDING` are queued without rewriting `model.json`; interrupted `RENDERING` models are reset to `PENDING` before being queued. Reconciliation is serialized with repository mutations. Queue limits, renderer timeout, output limits, and subprocess behavior are explicit runtime configuration.

## Projection Semantics

The CadQuery HLR adapter passes an explicit OpenCascade `gp_Ax2` direction and screen-right basis. This fixes both view direction and roll. Stored images remain immutable, so corrected technical orientation appears only after a later successful source revision.

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

`view.inspect` first requires a current successful revision and an exact current named view. A cache miss loads that revision's GLB and requests one bounded worker render. The server deduplicates concurrent work by revision, view ID, and view etag. It never executes source or launches Chromium in the application process.

Before an immutable cache write, the repository rechecks the current successful revision and view etag under its mutation lock. It repeats the identity check before return. A changed revision or etag produces a conflict and prevents mismatched output. Obsolete immutable cache objects can remain unreachable; no request can select them through a current identity.

## Security Boundary

MVP source is trusted operational input but remains capable of arbitrary Python behavior. The renderer is not a hostile-code sandbox. Production source access for untrusted principals is forbidden until a separately reviewed isolation design exists.
