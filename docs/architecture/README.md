# Architecture

These documents define repository-implemented MVP behavior and identify implementation status where needed. They provide no evidence of a preview or production deployment.

| Document | Covers |
| --- | --- |
| [Overview](overview.md) | Components, request flows, trust boundaries, and exclusions. |
| [Protocol](protocol.md) | gRPC-web, MCP tools, internal visual RPC, view concurrency, and HTTP geometry. |
| [Model projects and dependencies](model-projects-dependencies.md) | V2 bundles, model releases, exact dependency closure, MCP operations, and compatible rollout. |
| [Multipart design bundles](design-bundles.md) | Versioned Python results, named outputs, primary selection, compatibility, and exclusions. |
| [Storage and rendering](storage-rendering.md) | Garage keys, render recipes, project revision state, and render caches. |
| [Access and authentication](access-authentication.md) | Browser and MCP authentication, visual-worker isolation, and data boundaries. |
| [Observability and browser telemetry](observability.md) | Server logs, OTLP traces and metrics, Pyroscope profiling, and Faro collection. |
