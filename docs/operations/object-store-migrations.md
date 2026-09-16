# Object-Store Migrations

## Scope

Object-store migrations change durable repository formats before normal runtime code reads them. They do not authorize deployment or a live migration.

The permanent migration sequence contains `0001-model-project-bundles` followed by `0002-model-dependency-cutover`. Migration `0001` retains its identity and historical implementation. Migration `0002` deliberately deletes all current product data instead of converting it.

[`../architecture/model-projects-dependencies.md`](../architecture/model-projects-dependencies.md) defines the v2 project and model-release contract. [`../architecture/design-bundles.md`](../architecture/design-bundles.md) defines multipart outputs created after the cutover.

## Framework and Ordering

Each migration has one permanent identifier that matches `^[0-9]{4}-[a-z0-9]+(-[a-z0-9]+)*$`. A compile-time registry lists modules in ascending numeric-prefix order. Identifiers cannot be reused, reordered, rewritten, or removed.

The single server process owns migration execution. At startup, it reads the durable migration ledgers and runs each missing migration in order. It rejects an unknown completed identifier. A binary whose migration registry ends before a completed ledger fails closed. Therefore, an old binary cannot start after `0002` completes.

Migrations finish before repository validation, model enumeration, render reconciliation, API readiness, or request service. A failed migration keeps readiness false. It prevents later migrations and render work.

Each migration tolerates restart at every write boundary. A migration writes its immutable completion ledger only after all mutation and verification steps succeed. Completion ledgers live at `system/migrations/{migration_id}.json`. MCP, protobuf, browser APIs, logs, and telemetry never expose migration object contents.

## Permanent 0001 History

Migration `0001-model-project-bundles` converts the former one-file format to project bundles. Its completion ledger remains permanent. Its historical per-model checkpoints and recovery copies use these prefixes:

```text
system/migrations/0001-model-project-bundles/items/
system/migrations/0001-model-project-bundles/backup/
```

Migration `0002` deletes those obsolete product-data objects. It preserves `system/migrations/0001-model-project-bundles.json` and every other completion ledger.

The target contract does not retain `0001` conversion output, rollback data, or compatibility behavior. Those objects exist only until `0002` removes them.

## Destructive 0002 Cutover

Migration `0002-model-dependency-cutover` is an authorized, irreversible product-data reset. Current product data has no preservation requirement. The migration performs no conversion and creates no backup.

It deletes every object under these prefixes:

```text
models/
libraries/
system/library-rollouts/
system/migrations/0001-model-project-bundles/items/
system/migrations/0001-model-project-bundles/backup/
```

It never deletes any `system/migrations/{migration_id}.json` completion ledger.

The migration uses this restart-safe sequence:

1. Enumerate every target prefix with complete pagination.
2. Delete each listed object with missing-object success semantics.
3. Repeat enumeration and deletion until every target prefix returns empty.
4. Verify each target prefix through a fresh complete listing.
5. Write the immutable `system/migrations/0002-model-dependency-cutover.json` completion ledger.

A process crash can leave a partial deletion. Restart repeats the same sequence and converges on empty target prefixes. The migration needs no per-item checkpoint because deletion is idempotent. A list or delete error stops the migration and keeps readiness false.

The completion ledger cannot exist unless all target prefixes are empty. An existing completion ledger skips deletion under normal startup rules. Repository validation then accepts only `faktory-project-v2` bundles and the model-release storage contract.

## Recovery and Rollback Boundary

The deletion has no rollback path. Migration `0002` creates no recovery copy, compatibility alias, object remap, or conversion output. Operators must not repoint metadata, reconstruct deleted objects, or remove the completion ledger.

Recovery from interruption means restart with the same or a newer binary and complete the deletion. Recovery from a software defect means deploy a corrected newer binary that recognizes the immutable ledger sequence. An older binary fails closed when it sees the newer completion ledger.

Any live deployment, migration execution, object deletion, ledger repair, or restore requires separate target-specific authorization. This documentation grants no external action authority. No external deployment or migration execution is authorized by this cutover specification.

## Framework Rules

- Migrations are deterministic, bounded by complete pagination, ordered, and restart-safe.
- One migration module owns one transition. A fix ships as a later migration instead of changing a completed module.
- Migration errors expose only safe counts, identifiers, and bounded summaries.
- The single-server-replica constraint applies throughout migration and service.
