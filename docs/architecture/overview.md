# Architecture Overview

## Status

The repository implements the model-dependency hard cutover and multipart design-bundle contract. The implementation includes v2 projects, immutable model releases, exact transitive dependency closure, compatible release rollout, and destructive migration `0002`. Preview and production declarations include the isolated visual renderer, object buckets, database backups, and telemetry destinations. Repository sources do not prove a live collector, bucket, route, Authentik resource, pipeline run, preview deployment, or production deployment.

## MVP Components

| Component | Responsibility |
| --- | --- |
| Rust server | Own authentication boundaries, model metadata, view mutations, render coordination, gRPC-web, HTTP artifacts, and MCP hosting. |
| Solid SPA | Show one catalog row per model, lazily load previews, load GLB geometry, show semantic facts and render state, and edit shared views. |
| Garage | Persist model metadata, immutable v2 projects, model releases, revision artifacts, rollout and migration state, recipe-versioned shaded images, and named-view JSON objects. |
| CadQuery renderer | Materialize one trusted project and its exact model dependency closure, execute the root entrypoint, and produce each declared output. |
| Visual renderer | Render primary-only canonical and named-view shaded PNGs through isolated Node, Playwright, Chromium, and the shared Three.js recipe. |
| Authentik | Authenticate production browser users for server-owned sessions. |
| Hosted MCP OAuth | Authenticate and authorize production MCP clients independently of browser sessions. |
| Telemetry integrations | Emit bounded server logs, traces, metrics, optional CPU profiles, and production-build browser telemetry. |

The MVP runs one server replica and one AMD64 visual-renderer replica. This avoids distributed render ownership and cross-replica watch coordination. Availability and horizontal scaling are not MVP guarantees.

## Data Flow

1. An MCP principal creates a model or applies file, entrypoint, dependency, or hint edits through MCP; production uses hosted OAuth, while disabled mode admits unauthenticated local access.
2. The server normalizes the project, resolves declared direct model releases, validates the exact transitive closure, regenerates `AGENTS.md`, hashes the canonical bundle, writes an immutable project revision, and records it as desired.
3. The CadQuery renderer evaluates the entrypoint, normalizes its result, and produces each declared output's GLB, preview, facts, and technical projections.
4. The server rasterizes every technical set and sends only the primary GLB for seven canonical shaded PNGs.
5. The server advances the current successful revision only after the manifest and every required artifact persist. It stores all output summaries and reports `READY`.
6. On any output failure, the server preserves the complete prior successful design bundle and reports `FAILED` for the desired revision.
7. Browser clients receive metadata through gRPC-web and retrieve current-successful artifacts through HTTP, subject to the active access mode.
8. Web users create and edit shared named views through gRPC-web. MCP can render one exact saved view through the isolated visual renderer.

The catalog displays one model per row. Existing browser fields and artifact URLs remain primary-output aliases. Models without successful geometry display unavailable facts.

## Exclusions

Browser source editing, model deletion, release mutation, release yanking, prerelease versions, external dependency installation, CQGI parameters, hostile-code sandboxing, native clients, progressive mesh streaming, and multi-replica rendering are excluded. Structured server constraints, partial output success, manufacturing exports, and per-output named views are also excluded. The fixed renderer runtime and Python standard library remain available to trusted code. Native ARM64 visual-renderer support also remains excluded. The trusted-source assumption is an explicit MVP limitation, not a security sandbox.

[`model-projects-dependencies.md`](model-projects-dependencies.md) defines project identity, model releases, exact dependency closure, MCP file operations, and compatible rollout. [`design-bundles.md`](design-bundles.md) defines multipart results. [`storage-rendering.md`](storage-rendering.md) defines replacement rendering.
