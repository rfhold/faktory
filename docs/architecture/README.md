# Architecture

These documents define MVP behavior implemented in the repository, including the completed model-project hard cutover. They provide no evidence of a preview or production deployment.

| Document | Covers |
| --- | --- |
| [Overview](overview.md) | Components, request flows, trust boundaries, and exclusions. |
| [Protocol](protocol.md) | gRPC-web, MCP tools, internal visual RPC, view concurrency, and HTTP geometry. |
| [Model projects and shared libraries](model-projects-libraries.md) | Canonical bundles, managed AGENTS.md, MCP file operations, immutable libraries, and compatible rollout. |
| [Storage and rendering](storage-rendering.md) | Garage keys, render recipes, project revision state, and render caches. |
| [Access and authentication](access-authentication.md) | Browser and MCP authentication, visual-worker isolation, and data boundaries. |
| [Observability and browser telemetry](observability.md) | Server logs, OTLP traces and metrics, Pyroscope profiling, and Faro collection. |
