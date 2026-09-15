# Documentation

This index routes readers to authoritative Faktory MVP contracts implemented in the repository. Repository implementation and declarations do not authorize deployment, and they provide no evidence of a preview or production deployment or pipeline run.

| Document | Covers |
| --- | --- |
| [Architecture](architecture/README.md) | System, protocol, storage, access, observability, profiling, and browser telemetry contracts. |
| [Operations](operations/README.md) | Runtime recovery, telemetry operation, storage, deployment, and release procedures. |
| [Quality](quality/README.md) | Focused and full local checks, required coverage, and evidence boundaries. |

The protobuf source is the wire authority. Architecture documents define behavior not expressible in protobuf, including authentication, HTTP geometry delivery, object layout, concurrency, and failure semantics.
