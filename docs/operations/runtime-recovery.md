# Runtime and Recovery

## Status

The server, renderer, authentication modes, MCP issuer, production image, and local Garage and PostgreSQL topology are implemented in the repository. Preview and production stacks declare separate artifact and backup buckets, scheduled database backups, and telemetry targets. No declaration or local test proves that a production bucket, backup, collector, Authentik resource, route, pipeline run, or deployment exists.

## Topology

The MVP has one Rust server replica and a bounded local render worker. Production uses external Ceph S3, Authentik, PostgreSQL, and ingress dependencies. Compose substitutes single-node Garage and local PostgreSQL services, sets `FAKTORY_AUTH_MODE=disabled`, and publishes Faktory only on `127.0.0.1:8080`. It must never be exposed beyond loopback. The single replica is a correctness constraint: do not scale it horizontally without a design for render leases, metadata concurrency, and watch fanout.

Preview and release delivery build separate Linux AMD64 and ARM64 images from committed conda-forge explicit locks, then publish a multi-architecture manifest only after both builds succeed. The Docker build rejects unsupported architectures, performs no dependency solve, and lets Compose select the host's supported architecture. [`deployment-releases.md`](deployment-releases.md) owns deployment and release behavior.

## Startup Reconciliation

At startup, the server enumerates authoritative model metadata. Every model in `PENDING` or `RENDERING` is returned to `PENDING` and queued once, including an explicit same-source rerender whose desired and current successful revisions match. A `FAILED` desired revision is not retried automatically. Operators can use render retry, or an MCP source edit can create new work.

Missing desired source objects or invalid metadata make the affected model unhealthy and must produce safe diagnostics. A current revision without its GLB, preview, or matching facts is also unhealthy. Reconciliation must not delete objects or advance the current successful revision.

## Failure Handling

- Render failure preserves the last successful GLB, preview, and facts. It reports `FAILED` for the desired revision.
- Process restart may interrupt rendering; startup reconciliation retries only interrupted pending work.
- Garage unavailability prevents authoritative mutation and artifact retrieval. The server must not advance success before both artifact writes complete.
- Watch disconnection is recovered by reconnecting and accepting a new authoritative snapshot.
- View etag conflicts are user-visible concurrency conflicts, not automatic overwrite opportunities.

## Backup and Restore Boundary

CloudNativePG declarations send gzip-compressed base backups and WAL to the backup bucket under `database`; the immediate and scheduled policy is defined in [`deployment-releases.md`](deployment-releases.md). The artifact bucket is separate and is not copied by the database backup declaration. A complete recovery therefore needs a usable database backup and the artifact bucket contents that its records reference.

OAuth signing records in PostgreSQL depend on the retained wrapping keys in the `faktory-oauth-wrapping-keys` Secret. Treat the database and every referenced key version as one recovery unit. A restored database cannot use an encrypted signing record if its wrapping-key version is absent. Follow [`oauth-wrapping-key-rotation-recovery.md`](oauth-wrapping-key-rotation-recovery.md) for staged rotation and paired recovery.

No automatic restore resource or disaster-recovery workflow is declared. Before an authorized restore, identify the target stack, verify the selected database recovery point and artifact availability through target-specific evidence, preserve the current resources, and prepare a reviewed CloudNativePG recovery declaration. Do not infer recoverability from retention settings or mock tests. Restore, bucket mutation, and destructive replacement require explicit target authorization and a rollback boundary.

## External Action Boundary

Compose startup is explicitly a local operation. It initializes development-only bucket and database state but no identity or OAuth state. The committed Compose service sets `FAKTORY_DEPLOYMENT_ENVIRONMENT=local` and omits OTLP and Pyroscope endpoints, so telemetry remains stdout-only. Production bucket creation, credential creation, Authentik configuration, OAuth registration, deployment, restore, model mutation, render execution, and live verification each require separate target-specific authorization. Repository documentation and manifests grant no authority to perform those external actions.
