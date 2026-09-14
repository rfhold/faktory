# Storage and Rendering

## Object Layout

The runtime storage adapter uses one S3-compatible artifact bucket with these exact keys. Local Compose supplies Garage 2.3 as that object store:

```text
models/{model_id}/model.json
models/{model_id}/revisions/{source_sha256}/source.py
models/{model_id}/revisions/{source_sha256}/model.glb
models/{model_id}/revisions/{source_sha256}/preview.svg
models/{model_id}/views/{view_id}.json
```

`model_id` is a caller-supplied, immutable identifier. It contains at most 64 ASCII bytes and matches `^[a-z0-9]+(-[a-z0-9]+)*$`. View IDs and etags are server-issued opaque UUIDs. `source_sha256` is the lowercase hexadecimal SHA-256 of the exact accepted source bytes. Revision source objects are immutable. Each successful revision owns immutable `model.glb` and `preview.svg` objects. Model and view creation and all immutable writes retain the object store's atomic `If-None-Match: *` request. For matched mutable `model.json` and view JSON updates and deletes, the process serializes mutations, reads the current object ETag, rejects a mismatch, and then sends an unconditional mutation. This is process-local optimistic concurrency, not distributed atomic compare-and-swap; it is safe only while the deployment enforces one server replica with a `Recreate` strategy.

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
3. Preserve the current successful revision, GLB, preview, and facts while the replacement renders.
4. Compute facts from the CadQuery result before glTF export. Sum compound or assembly component volumes without a boolean union, so overlaps can count independently.
5. Compute source-coordinate axis-aligned dimensions before glTF export, then produce and validate temporary GLB and SVG outputs.
6. Store immutable `model.glb` and `preview.svg` only after all outputs and facts pass validation.
7. After both writes succeed, atomically advance the current successful revision, replace its facts, clear the safe error, and set `READY`.
8. On any failure, preserve the prior current successful revision, artifacts, and facts. Set the desired revision state to `FAILED` and record only a safe bounded error.

The one server replica uses an in-process bounded rendering queue. On restart, models left `PENDING` are queued without rewriting `model.json`; interrupted `RENDERING` models are reset to `PENDING` before being queued. Reconciliation is serialized with repository mutations. Queue limits, renderer timeout, output limits, and subprocess behavior are explicit runtime configuration.

## Security Boundary

MVP source is trusted operational input but remains capable of arbitrary Python behavior. The renderer is not a hostile-code sandbox. Production source access for untrusted principals is forbidden until a separately reviewed isolation design exists.
