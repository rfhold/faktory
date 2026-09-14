# Documentation

This index routes readers to the authoritative Faktory MVP contracts. Runtime behavior and preview, production, backup, observability, and delivery declarations are implemented in the repository. These declarations do not authorize deployment, and no preview or production deployment or pipeline run is evidenced here.

| Document | Covers |
| --- | --- |
| [Architecture](architecture/README.md) | System, protocol, storage, access, observability, profiling, and browser telemetry contracts. |
| [Operations](operations/README.md) | Runtime recovery, telemetry operation, storage, deployment, and release procedures. |
| [Quality](quality/README.md) | Focused and full local checks, required coverage, and evidence boundaries. |

The protobuf source is the wire authority. Architecture documents define behavior not expressible in protobuf, including authentication, HTTP geometry delivery, object layout, concurrency, and failure semantics.
