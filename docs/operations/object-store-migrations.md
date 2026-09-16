# Object-Store Migrations

## Scope

Object-store migrations convert durable repository formats before normal runtime code reads them. They do not authorize a live migration. The first migration converts legacy one-file model revisions into the canonical project bundles defined by [`../architecture/model-projects-libraries.md`](../architecture/model-projects-libraries.md).

The multipart output contract does not add a destructive migration. [`../architecture/design-bundles.md`](../architecture/design-bundles.md) and the compatibility rules below define how the repository interprets revisions without an output manifest.

## Framework and Ordering

Each migration is one source module with one permanent identifier matching `^[0-9]{4}-[a-z0-9]+(-[a-z0-9]+)*$` and one description. The legacy conversion is `0001-model-project-bundles`. A single compile-time registry lists modules in ascending numeric-prefix order. Identifiers are never reused, reordered, or removed. The one server process is the sole migration owner. At startup it reads the durable migration ledger, rejects an unknown completed identifier or a binary older than the ledger, and runs every missing migration in order.

Migrations finish before repository schema validation, model enumeration, render reconciliation, API readiness, or request serving. A failed migration keeps readiness false and prevents later migrations and rendering. Each migration must tolerate restart at every write boundary. Immutable writes accept existing identical bytes, mutable updates use the repository's guarded mutation path, and a durable completion record is written only after every item and verification step succeeds.

The ledger lives under `system/migrations/{migration_id}.json`. Per-item checkpoints live under `system/migrations/{migration_id}/items/{model_id}.json`; recovery backups live under `system/migrations/{migration_id}/backup/{model_id}/`. These keys are implementation authority for migration state and are not exposed through MCP, protobuf, browser APIs, logs, or telemetry.

## Legacy Project Migration

The legacy migration performs these steps for each model in deterministic model-ID byte order:

1. Read and validate the legacy `model.json`, every referenced `source.py`, every immutable artifact required by the recorded state, and named-view metadata without changing current records.
2. Write a recovery backup containing the exact pre-migration `model.json` and an inventory of every legacy object key, ETag, size, and SHA-256. Copy each referenced legacy `source.py` into the backup. Never include source bytes in diagnostics.
3. For each referenced legacy revision, create a project containing `source.py` as its entrypoint, no library requirements or locks, generated `# Index` and empty `# Dependency Guidance` sections, and an empty `# Hints` section. Canonicalize it and write the immutable project bundle under its new project revision.
4. Copy, rather than move, each existing GLB, preview, technical projection, canonical shaded render, and named-view render-cache object to a key under the new revision. Require the complete serving set for the current-successful revision. Verify every copied object's bytes and metadata needed for serving before recording the old-to-new revision mapping.
5. Write migrated model metadata that remaps desired and current-successful revisions together with every artifact reference. Preserve display name, render state, safe error, facts, timestamps, views, default view, and the distinction between desired and last-good revisions.
6. Re-read the migrated model and all selectable desired and current-successful objects through the new repository format. Then mark the model checkpoint complete.
7. After every model checkpoint is complete and verified, mark the migration complete. Normal repository validation and render reconciliation may then start.

Orphaned immutable legacy objects are inventoried but do not become selectable. Existing legacy objects, backups, and source files are never deleted. Copy collisions accept byte-identical objects and fail closed on different bytes. On restart, the migration recognizes both legacy and already-migrated model metadata and verifies the durable backup and revision mapping before continuing. A failed model leaves either its original metadata or its fully written migrated metadata authoritative; no partial metadata object is accepted. No migration queues a render merely to convert storage.

## Multipart Manifest Compatibility

A project revision that lacks `outputs.json` is a valid legacy artifact revision when its required fixed-key serving set passes current validation. The repository exposes that set as one synthetic primary output with ID `primary`, role `assembly`, and the recorded legacy facts. Primary HTTP aliases, output-aware routes for `primary`, technical inspection, canonical shaded inspection when present, and named-view renders all select those fixed keys.

The repository returns not found for every other output ID on that revision. Startup does not create `outputs.json`, copy fixed keys into `outputs/primary/`, alter metadata, enqueue a render, or delete an object. A later successful project revision writes only the multipart layout. Current-successful advancement then switches the complete selectable set atomically.

## Last-Good and Retry Behavior

Migration does not collapse desired and current-successful identity. If they differed before startup, each legacy revision maps independently, the migrated desired revision retains its state, and the migrated current-successful project keeps its complete serving artifacts and facts. Failed and interrupted renders remain failed or pending according to their prior state. Later startup reconciliation applies only after migration completion.

Render retry uses the migrated desired bundle and exact empty or populated lock set. Artifact HTTP and MCP inspection continue to select only the remapped current-successful revision. Migration errors are safe and bounded and do not reveal source, paths inside project files, dependency guidance, object keys, or artifact bytes.

## Recovery and Rollback Boundary

The migration is forward-only after any model metadata points at a project revision. Rolling back to a binary that understands only legacy source revisions is unsupported, even though legacy objects remain. Before deploying the cutover binary, take and verify a database backup and an independent artifact-bucket recovery copy, record the pre-migration image digest, and stop all other writers.

Before metadata cutover, an operator may stop the new binary and return to the old binary without object deletion. After metadata cutover begins, recovery means restoring both database and artifact storage to the same verified pre-migration point or completing the forward migration with a corrected newer binary. Never reconstruct rollback state by deleting project objects or manually repointing individual models. Any live backup, restore, bucket copy, deployment, or retry requires explicit target authorization.

## Framework Rules

- Migrations are deterministic, bounded, ordered, observable by safe counts and identifiers, and idempotent under restart.
- One migration module owns one format transition; fixes ship as a later migration instead of rewriting a completed module.
- Migration code does not delete, yank, compact, render, resolve dependencies, or call external services.
- The single-server-replica constraint applies throughout migration; multi-replica migration and serving are unsupported.
