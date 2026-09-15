# Architecture

These documents define MVP behavior implemented by the server, renderer, SPA, MCP host, and infrastructure declarations. They are not evidence of an external deployment.

| Document | Covers |
| --- | --- |
| [Overview](overview.md) | Components, request flows, trust boundaries, and exclusions. |
| [Protocol](protocol.md) | gRPC-web, MCP inspect tools, internal visual RPC, view concurrency, and HTTP geometry. |
| [Storage and rendering](storage-rendering.md) | Garage keys, render recipes, projection semantics, revision state, and render caches. |
| [Access and authentication](access-authentication.md) | Browser and MCP authentication, visual-worker isolation, and data boundaries. |
| [Observability and browser telemetry](observability.md) | Server logs, OTLP traces and metrics, Pyroscope profiling, and Faro collection. |
