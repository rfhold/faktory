# Faktory

Forge Any Kind of Thing; Our Resources Yield.

Faktory is a service for storing CadQuery model sources, rendering GLB geometry, and sharing named camera views through a Solid browser application and MCP.

The repository contains the MVP server, CadQuery renderer, Solid SPA, MCP tools, protocol bindings, and a production container image. It also contains an intentionally unauthenticated local Compose environment plus preview deployment declarations. Compose publishes Faktory on `0.0.0.0:8080` with public origin `http://172.16.1.40:8080`; use it only on a trusted private network behind the host firewall. Garage remains loopback-only, and PostgreSQL and the visual renderer have no host ports. Production uses Authentik browser sessions and hosted MCP OAuth. The implementation and local tests exist; no production infrastructure or external Authentik, Ceph, PostgreSQL, Kubernetes, or cloud resource has been configured or deployed from this repository.

Start with the [documentation index](docs/README.md). Protocol consumers should use [the protobuf source](proto/faktory/v1/faktory.proto) and [the Rust protocol crate](crates/faktory-proto/). See [CONTRIBUTING.md](CONTRIBUTING.md) for validation and local Compose commands.
