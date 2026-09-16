# Access and Authentication

## Status

Explicit disabled and production authentication paths are implemented. The output-aware artifact routes reuse the same boundary under the approved multipart contract. Compose sets exact `FAKTORY_AUTH_MODE=disabled` and contains no identity provider. Pulumi declares the production Authentik application/provider and externally supplied secrets. No Authentik resource, OAuth client, session key, credential, or route has been created externally.

## Browser Access

In production, Authentik authenticates browser users. The Faktory server owns the resulting browser session and applies it to the static SPA, gRPC-web, and HTTP geometry requests. Browser clients do not send Authentik access tokens directly to application APIs.

Production requires HTTPS for the public URL and OIDC issuer. Userinfo, query strings, and fragments remain invalid. Missing `FAKTORY_AUTH_MODE` selects production rather than disabled mode, and missing or invalid production settings fail startup.

Disabled mode is accepted only through exact `FAKTORY_AUTH_MODE=disabled`. It applies no browser middleware or interceptor to the SPA, gRPC-web, or artifact routes, and serves MCP without OAuth or bearer authentication. Compose publishes this unauthenticated service on `0.0.0.0:8080` for `http://172.16.1.40:8080`. Use it only on a trusted private network, and restrict TCP 8080 with the host firewall. Garage remains loopback-only, and the visual renderer remains unexposed.

Every authenticated web user may list models, inspect render state, and retrieve every current-successful output's GLB and preview. Users may also list shared views, mutate them, and select a default view. Views target only the primary output and remain shared model state rather than per-user preferences.

## MCP Access

In production, the Faktory service is the hosted OAuth authorization server for MCP clients and uses Authentik for interactive user authentication. Authentik registers the strict browser callback `/oidc/callback` and hosted MCP resource-owner callback `/oauth/oidc/callback`. The shared resource-owner flow derives its login route as `/oauth/oidc/login` and resumes the owned `/oauth/authorize` endpoint after authentication. Dynamic client registration is enabled, including loopback redirect URIs required by local CLI clients such as OpenCode. MCP access tokens are Faktory-issued and resource-bound; browser sessions and Authentik tokens are not accepted as MCP bearer tokens. Disabled mode bypasses the hosted OAuth flow and redirect validation for local Compose. Absolute callback, authorization, and protected-resource metadata URLs still derive from `FAKTORY_PUBLIC_BASE_URL`, which Compose sets to `http://172.16.1.40:8080`.

Client ID Metadata Documents (CIMD), dynamic client registration, and loopback redirects use separate policy switches. `FAKTORY_OAUTH_ALLOW_CIMD` defaults to `false`; DCR or loopback support does not enable CIMD. The Pulumi program explicitly enables all three policies for its hosted preview and production declarations.

When CIMD is enabled, Faktory can retrieve a client metadata document from an HTTPS URL supplied as the OAuth `client_id`. The shared hardened fetcher disables redirects, applies time and response-size bounds, pins requests to approved DNS results, and rejects non-public destinations by default. `FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS` can name a bounded comma-separated set of credential-free HTTPS root origins. Only exact listed origins can resolve to private addresses. Faktory declares no trusted private CIMD origin because this repository proves no required private metadata endpoint. These declarations and runtime controls do not prove live metadata retrieval.

Pulumi creates one 256-bit OAuth wrapping key for each configured version. The keyring names one active version and retains older versions for encrypted database records. Version identifiers use unique DNS labels. The configuration accepts from 1 through 32 versions. Pulumi stores key material as secret state and projects it only through the `faktory-oauth-wrapping-keys` Secret. A secret-derived checksum rolls the server pod after a keyring change. The runtime rejects a keyring that cannot decrypt required OAuth signing state.

Any authenticated MCP principal can create, open, discover, read, patch, and edit model projects under the `faktory:use` scope. The same principal can list, inspect, and publish immutable model releases. The principal can inspect one selected output's technical projection per call. Shaded projection and saved-view inspection remain primary-only. The MVP defines no narrower source-author or publisher role. Project source, dependency source, generated guidance, requirements, locks, and project docs remain MCP-only. Workspace tools expose canonical objects, not server filesystem paths or a second storage model. MCP tools do not expose arbitrary object keys, Python execution arguments, renderer commands, storage credentials, GLB bytes, or storage-internal metadata. PNG bytes appear only in authorized semantic inspect results. Browser artifact URLs and browser sessions do not apply to MCP. [`protocol.md`](protocol.md) defines tool results and errors. [`model-projects-dependencies.md`](model-projects-dependencies.md) defines project, dependency, and release semantics.

## Visual Renderer Boundary

The visual renderer contains Node, Playwright, Chromium, and the render harness. It contains no SPA, Faktory authentication path, Rust server, or CadQuery runtime. The service exposes unauthenticated RPC only inside the cluster. A strict NetworkPolicy admits port 8081 only from the Faktory server pod and denies all worker egress. The worker therefore relies on network isolation for caller authentication.

For each job, the worker holds the GLB in memory behind a random loopback-only harness URL. It does not persist the model. Browser interception permits only the job page, model URL, and harness asset. It blocks all external requests, and a GLB with external dependencies fails.

Chromium retains its own sandbox. The launch config enables `chromiumSandbox` and does not pass `--no-sandbox`. The worker pod alone uses outer seccomp `Unconfined`. Kubernetes `RuntimeDefault` and a narrower tested profile deny syscalls or `chroot` that Chromium needs for its sandbox. This exception does not disable the Chromium sandbox and does not apply to the Faktory server pod, which retains `RuntimeDefault`.

The worker runs as non-root UID and GID 65532. Its pod disables service-account token mounts, prevents privilege escalation, drops all capabilities, uses a read-only root filesystem, and mounts a 256 MiB memory-backed `/tmp`. The declaration starts one replica on AMD64. Native ARM64 worker behavior remains unknown and unsupported.

## Data Safety

Credentials, project or dependency source, AGENTS content, requirements, locks, project docs, GLB, SVG, PNG bytes, session identifiers, authorization codes, tokens, object-store internals, and raw renderer output must not enter logs, traces, metrics, protobuf errors, or browser diagnostics. Only authorized MCP model tools intentionally return source text. `model.open` and model-release metadata results omit indexed file bodies. Bounded read and search tools return only requested content. Authorized `model.inspect` and `view.inspect` results provide the only intentional PNG responses. Render, rollout, and migration failures expose only safe bounded summaries.

Browser Faro telemetry is the explicit exception to this strict server-signal contract. Its accepted collection and privacy boundary are defined in [`observability.md`](observability.md); the server exclusions above must not be used to imply that standard Faro collection is globally sanitized.
