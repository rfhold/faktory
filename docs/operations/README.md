# Operations

These documents define MVP operation for repository implementations and declarations, including the destructive v2 cutover and model-release rollout recovery. They provide no evidence that a collector, bucket, route, pipeline, preview stack, or production stack exists live.

| Document | Covers |
| --- | --- |
| [Runtime and recovery](runtime-recovery.md) | Server and visual-worker topology, reconciliation, render recovery, and external-action gates. |
| [Object-store migrations](object-store-migrations.md) | Permanent ordering, destructive `0002` deletion, restart safety, and forward-only startup. |
| [OAuth wrapping-key rotation and recovery](oauth-wrapping-key-rotation-recovery.md) | Staged keyring rotation, rollback, and paired database recovery. |
| [Observability and profiling](observability-profiling.md) | Configuration, expected signals, shutdown, and troubleshooting. |
| [Deployment and releases](deployment-releases.md) | Stack declarations, backups, signed release flow, downtime, and manual rollback. |
