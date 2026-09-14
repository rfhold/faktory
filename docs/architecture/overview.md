# Architecture Overview

## Status

The server, Solid SPA, MCP host and tools, CadQuery renderer, S3 adapter, production image, and local Compose topology are implemented in the repository. Preview and production declarations cover object buckets, database backups, and telemetry destinations. Local tests cover those declarations. Delivery pipeline declarations also exist. No collector, bucket, route, Authentik resource, pipeline run, preview deployment, or production deployment is evidenced by these declarations.

## MVP Components

| Component | Responsibility |
| --- | --- |
| Rust server | Own authentication boundaries, model metadata, view mutations, render scheduling, gRPC-web, HTTP artifacts, and MCP hosting. |
| Solid SPA | Show one catalog row per model, lazily load previews, load GLB geometry, show semantic facts and render state, and edit shared views. |
| Garage | Persist model metadata, immutable revision sources, GLBs, SVG previews, and named-view JSON objects. |
| CadQuery renderer | Execute one trusted Python source file, compute geometry facts, and produce GLB and SVG outputs. |
| Authentik | Authenticate production browser users for server-owned sessions. |
| Hosted MCP OAuth | Authenticate and authorize production MCP clients independently of browser sessions. |
| Telemetry integrations | Emit bounded server logs, traces, metrics, optional CPU profiles, and production-build browser telemetry. |

The MVP runs exactly one server replica. This avoids distributed render ownership and cross-replica watch coordination. Availability and horizontal scaling are not MVP guarantees.

## Data Flow

1. An MCP principal creates a model or edits its source through MCP; production uses hosted OAuth, while disabled mode admits unauthenticated local access.
2. The server validates the caller-supplied model ID, hashes the UTF-8 source, writes an immutable source revision, and records it as desired.
3. The renderer evaluates the source, computes facts, and produces validated GLB and SVG outputs.
4. The server advances the current successful revision only after both artifacts persist. It also stores the matching facts and reports `READY`.
5. On failure, the server preserves the prior successful artifacts and facts, then reports `FAILED` for the desired revision.
6. Browser clients receive metadata through gRPC-web and retrieve current-successful artifacts through HTTP, guarded by the active access mode.
7. Web users create and edit shared named views through gRPC-web, with production requiring a browser session and disabled mode requiring no credentials.

The catalog displays one model per row. Each row lazily loads the current-successful preview and presents volume and x/y/z dimensions semantically. Models without successful geometry display unavailable facts.

## Exclusions

Browser source editing, model deletion, CQGI parameters, hostile-code sandboxing, native clients, progressive mesh streaming, and multi-replica rendering are excluded. The trusted-source assumption is an explicit MVP limitation, not a security sandbox.

[`storage-rendering.md`](storage-rendering.md) defines model identity, source creation, revision-guarded partial edits, and replacement rendering.
