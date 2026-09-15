# Runtime and Recovery

## Status

The server, CadQuery renderer, visual renderer, authentication modes, MCP issuer, and local Garage and PostgreSQL topology exist in the repository. Preview and production stacks declare the isolated visual-renderer workload, artifact and backup buckets, database backups, and telemetry targets. No declaration or local test proves that a production bucket, backup, collector, Authentik resource, route, pipeline run, or deployment exists.

## Topology

The MVP has one Rust server replica, one bounded in-process render queue, and one isolated AMD64 visual-renderer replica. Production uses external Ceph S3, Authentik, PostgreSQL, and ingress dependencies. The server requires `FAKTORY_VISUAL_RENDERER_URL` in production. Its default visual request timeout is 30 seconds.

Compose substitutes single-node Garage and local PostgreSQL services. It also builds one AMD64 visual renderer and configures its internal URL. Compose sets `FAKTORY_AUTH_MODE=disabled` and publishes Faktory only on `127.0.0.1:8080`. It does not publish the worker port. Compose must never receive traffic beyond loopback. The single server replica remains a correctness constraint. Do not scale it without designs for render leases, metadata concurrency, and watch fanout.

Preview and release delivery build separate Linux AMD64 and ARM64 Faktory server images from committed conda-forge explicit locks. They build and verify a separate AMD64 visual-renderer image. The Pulumi workload selects AMD64 nodes for that image. Native ARM64 visual-renderer behavior remains unknown and unsupported. [`deployment-releases.md`](deployment-releases.md) owns current deployment and release behavior.

## Startup Reconciliation

At startup, the server enumerates authoritative model metadata. Every model in `PENDING` or `RENDERING` is returned to `PENDING` and queued once, including an explicit same-source rerender whose desired and current successful revisions match. A `FAILED` desired revision is not retried automatically. Operators can use render retry, or an MCP source edit can create new work.

Missing desired source objects or invalid metadata make the affected model unhealthy and must produce safe diagnostics. A current revision without its GLB, preview, or matching facts is also unhealthy. Reconciliation must not delete objects or advance the current successful revision.

## Failure Handling

- Render failure preserves the last successful GLB, preview, technical images, shaded images, and facts. It reports `FAILED` for the desired revision.
- Process restart may interrupt rendering; startup reconciliation retries only interrupted pending work.
- Garage unavailability prevents authoritative mutation and artifact retrieval. The server advances success only after every required artifact write.
- Visual-renderer unavailability, timeout, crash, malformed output, or saturation fails the desired source render. Restart alone does not retry a `FAILED` revision.
- An operator can call `model.render.retry` for a failed desired revision after worker recovery. A source edit can also create new work.
- A visual-renderer crash can leave only worker-local temporary data. The read-only root and memory-backed `/tmp` make that data disposable.
- A failed named-view render leaves no selectable cache entry. Retry the same `view.inspect` after worker recovery.
- A view or revision race can leave an unreachable immutable cache object. Current identity checks prevent its return; no cache cleanup operation exists.
- A legacy current revision can lack canonical shaded images. No rerender or backfill operation exists for that gap; a later source revision must succeed.
- A saved-view inspect can render from a legacy current GLB after worker recovery, even when canonical shaded images remain absent.
- Watch disconnection is recovered by reconnecting and accepting a new authoritative snapshot.
- View etag conflicts are user-visible concurrency conflicts, not automatic overwrite opportunities.

## Backup and Restore Boundary

CloudNativePG declarations send gzip-compressed base backups and WAL to the backup bucket under `database`; the immediate and scheduled policy is defined in [`deployment-releases.md`](deployment-releases.md). The artifact bucket is separate and is not copied by the database backup declaration. A complete recovery therefore needs a usable database backup and the artifact bucket contents that its records reference.

OAuth signing records in PostgreSQL depend on the retained wrapping keys in the `faktory-oauth-wrapping-keys` Secret. Treat the database and every referenced key version as one recovery unit. A restored database cannot use an encrypted signing record if its wrapping-key version is absent. Follow [`oauth-wrapping-key-rotation-recovery.md`](oauth-wrapping-key-rotation-recovery.md) for staged rotation and paired recovery.

No automatic restore resource or disaster-recovery workflow is declared. Before an authorized restore, identify the target stack, verify the selected database recovery point and artifact availability through target-specific evidence, preserve the current resources, and prepare a reviewed CloudNativePG recovery declaration. Do not infer recoverability from retention settings or mock tests. Restore, bucket mutation, and destructive replacement require explicit target authorization and a rollback boundary.

## External Action Boundary

Compose startup is explicitly a local operation. It initializes development-only bucket and database state but no identity or OAuth state. The committed Compose services include the visual renderer. They set `FAKTORY_DEPLOYMENT_ENVIRONMENT=local` and omit OTLP and Pyroscope endpoints, so telemetry remains stdout-only. Production bucket creation, credential creation, Authentik configuration, OAuth registration, deployment, restore, model mutation, render execution, and live verification each require separate target-specific authorization. Repository documentation and manifests grant no authority to perform those external actions.
