# Architecture

These documents define MVP behavior implemented by the server, renderer, SPA, MCP host, and infrastructure declarations. They are not evidence of an external deployment.

| Document | Covers |
| --- | --- |
| [Overview](overview.md) | Components, request flows, trust boundaries, and exclusions. |
| [Protocol](protocol.md) | gRPC-web methods, watch ordering, view concurrency, and HTTP geometry. |
| [Storage and rendering](storage-rendering.md) | Garage keys, source contract, revision state, and render failures. |
| [Access and authentication](access-authentication.md) | Explicit local disabled mode, production Authentik browser sessions, hosted MCP OAuth, and shared authority. |
| [Observability and browser telemetry](observability.md) | Server logs, OTLP traces and metrics, Pyroscope profiling, and Faro collection. |
