# Runtime and Recovery

## Status

The server, CadQuery renderer, visual renderer, authentication modes, MCP issuer, object-store migrations, shared-library rollout recovery, and local Garage and PostgreSQL topology exist in the repository. The multipart recovery rules below define approved behavior under implementation. Preview and production stacks declare the isolated visual-renderer workload, artifact and backup buckets, database backups, and telemetry targets. No repository implementation or declaration proves that a production bucket, backup, collector, Authentik resource, route, pipeline run, preview deployment, or production deployment exists.

## Topology

The MVP has one Rust server replica, one bounded in-process render queue, and one isolated AMD64 visual-renderer replica. Production uses external Ceph S3, Authentik, PostgreSQL, and ingress dependencies. The server requires `FAKTORY_VISUAL_RENDERER_URL` in production. Its default visual request timeout is 30 seconds.

Compose substitutes single-node Garage and local PostgreSQL services. It also builds one AMD64 visual renderer and configures its internal URL. Compose sets `FAKTORY_AUTH_MODE=disabled`, publishes Faktory on `0.0.0.0:8080`, and sets `FAKTORY_PUBLIC_BASE_URL=http://172.16.1.40:8080`. The service accepts unauthenticated LAN traffic, so use it only on a trusted private network and restrict TCP 8080 with the host firewall. Garage stays on loopback. PostgreSQL and the worker have no host ports. Disabled mode bypasses OAuth redirect validation, but generated absolute URLs use the configured public base. The single server replica remains a correctness constraint. Do not scale it without designs for render leases, metadata concurrency, and watch fanout.

Preview and release delivery build separate Linux AMD64 and ARM64 Faktory server images from committed conda-forge explicit locks. They build and verify a separate AMD64 visual-renderer image. The Pulumi workload selects AMD64 nodes for that image. Native ARM64 visual-renderer behavior remains unknown and unsupported. [`deployment-releases.md`](deployment-releases.md) owns current deployment and release behavior.

## Startup Reconciliation

At startup, the server runs every ordered object-store migration to completion before repository validation or render reconciliation. [`object-store-migrations.md`](object-store-migrations.md) defines migration checkpoints, legacy project conversion, backups, and the forward-only rollback boundary. A migration failure keeps readiness false and queues no render work.

After migrations complete, the server enumerates authoritative model metadata. Every model in `PENDING` or `RENDERING` is returned to `PENDING` and queued once, including an explicit same-project rerender whose desired and current successful revisions match. A `FAILED` desired revision is not retried automatically. Operators can use render retry, or an MCP project edit can create new work. Durable compatible-library rollout records also resume idempotently and may create new desired project revisions.

Missing desired project bundles or exact locked releases and invalid metadata make the affected model unhealthy and must produce safe diagnostics. A multipart current revision is unhealthy if it lacks its manifest, any declared required artifact, output summaries, or matching facts. A legacy current revision remains valid without a manifest when its synthetic primary artifact set is complete. Reconciliation must not delete objects, backfill manifests, resolve dependencies, or advance the current successful revision.

## Failure Handling

- Any output render failure preserves the complete last-successful manifest, output artifacts, summaries, and facts. It reports `FAILED` for the desired revision.
- Process restart may interrupt rendering; startup reconciliation retries only interrupted pending work.
- Garage unavailability prevents authoritative mutation and artifact retrieval. The server advances success only after every required artifact write.
- Visual-renderer unavailability, timeout, crash, malformed output, or saturation fails the desired project render. Restart alone does not retry a `FAILED` revision.
- An operator can call `model.render.retry` for a failed desired revision after worker recovery. A project edit can also create new work.
- `model.render.retry` reuses the exact desired project and library locks. It never resolves a newly published release.
- Compatible-library rollout resumes from its durable record after interruption. It re-evaluates concurrently edited models and never overwrites a newer desired project.
- A visual-renderer crash can leave only worker-local temporary data. The read-only root and memory-backed `/tmp` make that data disposable.
- A failed primary named-view render leaves no selectable cache entry. Retry the same `view.inspect` after worker recovery.
- A view or revision race can leave an unreachable immutable cache object. Current identity checks prevent its return; no cache cleanup operation exists.
- A legacy current revision has one synthetic primary output and needs no manifest backfill. Its fixed keys remain selectable.
- A legacy current revision can lack canonical shaded images. No rerender or backfill operation exists for that gap; a later project revision must succeed.
- A saved-view inspect can render from a legacy primary GLB after worker recovery, even when canonical shaded images remain absent.
- Watch disconnection is recovered by reconnecting and accepting a new authoritative snapshot.
- View etag conflicts are user-visible concurrency conflicts, not automatic overwrite opportunities.

## Backup and Restore Boundary

CloudNativePG declarations send gzip-compressed base backups and WAL to the backup bucket under `database`; the immediate and scheduled policy is defined in [`deployment-releases.md`](deployment-releases.md). The artifact bucket is separate and is not copied by the database backup declaration. A complete recovery therefore needs a usable database backup and the artifact bucket contents that its records reference.

OAuth signing records in PostgreSQL depend on the retained wrapping keys in the `faktory-oauth-wrapping-keys` Secret. Treat the database and every referenced key version as one recovery unit. A restored database cannot use an encrypted signing record if its wrapping-key version is absent. Follow [`oauth-wrapping-key-rotation-recovery.md`](oauth-wrapping-key-rotation-recovery.md) for staged rotation and paired recovery.

No automatic restore resource or disaster-recovery workflow is declared. Before an authorized restore, identify the target stack, verify the selected database recovery point and artifact availability through target-specific evidence, preserve the current resources, and prepare a reviewed CloudNativePG recovery declaration. Do not infer recoverability from retention settings or mock tests. Restore, bucket mutation, and destructive replacement require explicit target authorization and a rollback boundary.

## External Action Boundary

Compose startup is explicitly a local operation. It initializes development-only bucket and database state but no identity or OAuth state. The committed Compose services include the visual renderer. They set `FAKTORY_DEPLOYMENT_ENVIRONMENT=local` and omit OTLP and Pyroscope endpoints, so telemetry remains stdout-only. Production bucket creation, credential creation, Authentik configuration, OAuth registration, deployment, migration, restore, model or library mutation, render execution, and live verification each require separate target-specific authorization. Repository documentation and manifests grant no authority to perform those external actions.
