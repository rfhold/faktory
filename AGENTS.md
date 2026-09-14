# Index

| Path | Info |
| --- | --- |
| [crates/faktory-proto/](crates/faktory-proto/) | Committed Prost and Tonic types for the versioned Faktory API. |
| [proto/](proto/) | Authoritative protobuf sources and generated browser bindings. |
| [docs/](docs/) | Indexed MVP architecture, operations, and quality contracts. |
| [compose.yaml](compose.yaml) | Deterministic local Faktory, Garage, and PostgreSQL integration topology. |
| [Dockerfile](Dockerfile) | Multi-stage production image for the server, renderer, and SPA. |
| [infra/pulumi/](infra/pulumi/) | Preview Kubernetes, PostgreSQL, Authentik, and networking declarations. |
| [.tekton/](.tekton/) | Preview and release image publication and Pulumi delivery pipelines. |
| [Cargo.toml](Cargo.toml) | Rust 1.96, edition 2024 workspace policy and shared dependencies. |
| [package.json](package.json) | pnpm workspace tooling and browser protobuf dependencies. |
| [pyproject.toml](pyproject.toml) | Python and CadQuery renderer environment contract. |
| [renderer/environment.yml](renderer/environment.yml) | Production-image conda input; committed explicit locks are under `renderer/conda/`. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Local generation and validation commands. |

# Hints

- Read [docs/README.md](docs/README.md) before changing behavior or contracts.
- The local MVP implementation, image, Compose environment, and preview declarations exist. No Faktory service is deployed and no external system is configured or mutated.
- Keep geometry transfer out of protobuf; authenticated HTTP owns GLB delivery.
- Treat `proto/faktory/v1/faktory.proto` as the API authority and regenerate committed bindings after changes.
- Preserve Rust 1.96, edition 2024, Buf v2 `STANDARD` lint, and `FILE` breaking policy.
- Keep uv authoritative for native renderer development and tests; image builds consume the committed conda explicit lock matching BuildKit `TARGETARCH`.
- Keep planned behavior distinct from implemented and deployed behavior in documentation.
- Do not add secrets, credentials, generated model artifacts, or local object-store data.
- Use `agentic-documentation` for documentation changes and `planning-changes` before runtime, storage, rendering, authentication, MCP, or delivery behavior changes.
