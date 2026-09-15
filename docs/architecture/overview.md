# Architecture Overview

## Status

The server, Solid SPA, MCP host and tools, CadQuery renderer, S3 adapter, and visual renderer exist in the repository. Preview and production declarations include the isolated visual renderer, object buckets, database backups, and telemetry destinations. Local tests cover those declarations. Delivery pipeline declarations also exist. These sources do not prove a live collector, bucket, route, Authentik resource, pipeline run, preview deployment, or production deployment.

## MVP Components

| Component | Responsibility |
| --- | --- |
| Rust server | Own authentication boundaries, model metadata, view mutations, render coordination, gRPC-web, HTTP artifacts, and MCP hosting. |
| Solid SPA | Show one catalog row per model, lazily load previews, load GLB geometry, show semantic facts and render state, and edit shared views. |
| Garage | Persist model metadata, immutable revision artifacts, recipe-versioned shaded images, and named-view JSON objects. |
| CadQuery renderer | Execute one trusted Python source file, compute facts, and produce GLB and technical SVG outputs. |
| Visual renderer | Render canonical and named-view shaded PNGs through isolated Node, Playwright, Chromium, and the shared Three.js recipe. |
| Authentik | Authenticate production browser users for server-owned sessions. |
| Hosted MCP OAuth | Authenticate and authorize production MCP clients independently of browser sessions. |
| Telemetry integrations | Emit bounded server logs, traces, metrics, optional CPU profiles, and production-build browser telemetry. |

The MVP runs one server replica and one AMD64 visual-renderer replica. This avoids distributed render ownership and cross-replica watch coordination. Availability and horizontal scaling are not MVP guarantees.

## Data Flow

1. An MCP principal creates a model or edits its source through MCP; production uses hosted OAuth, while disabled mode admits unauthenticated local access.
2. The server validates the caller-supplied model ID, hashes the UTF-8 source, writes an immutable source revision, and records it as desired.
3. The CadQuery renderer evaluates the source, computes facts, and produces validated GLB, preview SVG, and seven technical SVG outputs.
4. The server rasterizes the technical outputs and sends the GLB once to the visual renderer for seven canonical shaded PNGs.
5. The server advances the current successful revision only after every required artifact persists. It also stores the matching facts and reports `READY`.
6. On failure, the server preserves the prior successful artifacts and facts, then reports `FAILED` for the desired revision.
7. Browser clients receive metadata through gRPC-web and retrieve current-successful artifacts through HTTP, subject to the active access mode.
8. Web users create and edit shared named views through gRPC-web. MCP can render one exact saved view through the isolated visual renderer.

The catalog displays one model per row. Each row lazily loads the current-successful preview and presents volume and x/y/z dimensions semantically. Models without successful geometry display unavailable facts.

## Exclusions

Browser source editing, model deletion, CQGI parameters, hostile-code sandboxing, native clients, progressive mesh streaming, and multi-replica rendering are excluded. Native ARM64 visual-renderer support also remains excluded. The trusted-source assumption is an explicit MVP limitation, not a security sandbox.

[`storage-rendering.md`](storage-rendering.md) defines model identity, source creation, revision-guarded partial edits, and replacement rendering.
