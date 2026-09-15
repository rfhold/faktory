# Architecture Overview

## Status

The server, Solid SPA, multi-file project MCP tools, managed AGENTS.md, shared libraries, rollout worker, object-store migrations, CadQuery renderer, S3 adapter, and visual renderer exist in the repository. Preview and production declarations include the isolated visual renderer, object buckets, database backups, and telemetry destinations. Delivery pipeline declarations also use the project-root renderer CLI. These sources do not prove a live collector, bucket, route, Authentik resource, pipeline run, preview deployment, or production deployment.

## MVP Components

| Component | Responsibility |
| --- | --- |
| Rust server | Own authentication boundaries, model metadata, view mutations, render coordination, gRPC-web, HTTP artifacts, and MCP hosting. |
| Solid SPA | Show one catalog row per model, lazily load previews, load GLB geometry, show semantic facts and render state, and edit shared views. |
| Garage | Persist model metadata, immutable project and library bundles, revision artifacts, rollout and migration state, recipe-versioned shaded images, and named-view JSON objects. |
| CadQuery renderer | Execute one trusted multi-file Python project with exact direct-library locks, compute facts, and produce GLB and technical SVG outputs. |
| Visual renderer | Render canonical and named-view shaded PNGs through isolated Node, Playwright, Chromium, and the shared Three.js recipe. |
| Authentik | Authenticate production browser users for server-owned sessions. |
| Hosted MCP OAuth | Authenticate and authorize production MCP clients independently of browser sessions. |
| Telemetry integrations | Emit bounded server logs, traces, metrics, optional CPU profiles, and production-build browser telemetry. |

The MVP runs one server replica and one AMD64 visual-renderer replica. This avoids distributed render ownership and cross-replica watch coordination. Availability and horizontal scaling are not MVP guarantees.

## Data Flow

1. An MCP principal creates a model or applies file, entrypoint, dependency, or hint edits through MCP; production uses hosted OAuth, while disabled mode admits unauthenticated local access.
2. The server normalizes the project, resolves only declared direct libraries when required, regenerates `AGENTS.md`, hashes the canonical bundle, writes an immutable project revision, and records it as desired.
3. The CadQuery renderer evaluates the explicit entrypoint with exact locked libraries, computes facts, and produces validated GLB, preview SVG, and seven technical SVG outputs.
4. The server rasterizes the technical outputs and sends the GLB once to the visual renderer for seven canonical shaded PNGs.
5. The server advances the current successful revision only after every required artifact persists. It also stores the matching facts and reports `READY`.
6. On failure, the server preserves the prior successful artifacts and facts, then reports `FAILED` for the desired revision.
7. Browser clients receive metadata through gRPC-web and retrieve current-successful artifacts through HTTP, subject to the active access mode.
8. Web users create and edit shared named views through gRPC-web. MCP can render one exact saved view through the isolated visual renderer.

The catalog displays one model per row. Each row lazily loads the current-successful preview and presents volume and x/y/z dimensions semantically. Models without successful geometry display unavailable facts.

## Exclusions

Browser source editing, model deletion, library yanking or deletion, prerelease libraries, external dependency declaration or installation, transitive shared-library dependencies, CQGI parameters, hostile-code sandboxing, native clients, progressive mesh streaming, and multi-replica rendering are excluded. The fixed renderer runtime and Python standard library remain available to trusted code. Native ARM64 visual-renderer support also remains excluded. The trusted-source assumption is an explicit MVP limitation, not a security sandbox.

[`model-projects-libraries.md`](model-projects-libraries.md) defines project identity, MCP file operations, managed guidance, and compatible library rollout. [`storage-rendering.md`](storage-rendering.md) defines replacement rendering.
