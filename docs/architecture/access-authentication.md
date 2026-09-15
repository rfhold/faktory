# Access and Authentication

## Status

Explicit disabled and production authentication paths are implemented. Compose sets exact `FAKTORY_AUTH_MODE=disabled` and contains no identity provider. Pulumi declares the production Authentik application/provider and externally supplied secrets. No Authentik resource, OAuth client, session key, credential, or route has been created externally.

## Browser Access

In production, Authentik authenticates browser users. The Faktory server owns the resulting browser session and applies it to the static SPA, gRPC-web, and HTTP geometry requests. Browser clients do not send Authentik access tokens directly to application APIs.

Production requires HTTPS for the public URL and OIDC issuer. Userinfo, query strings, and fragments remain invalid. Missing `FAKTORY_AUTH_MODE` selects production rather than disabled mode, and missing or invalid production settings fail startup.

Disabled mode is accepted only through exact `FAKTORY_AUTH_MODE=disabled`. It applies no browser middleware or interceptor to the SPA, gRPC-web, or artifact routes, and serves MCP without OAuth or bearer authentication. This mode is intended only for the Compose port published on loopback and must never be exposed beyond the local machine.

Every authenticated web user may list models, inspect render state, retrieve current successful geometry, list shared views, create or update a named view, delete a named view, and select a default view. Views are shared model state rather than per-user preferences.

## MCP Access

In production, the Faktory service is the hosted OAuth authorization server for MCP clients and uses Authentik for interactive user authentication. Authentik registers the strict browser callback `/oidc/callback` and hosted MCP resource-owner callback `/oauth/oidc/callback`. The shared resource-owner flow derives its login route as `/oauth/oidc/login` and resumes the owned `/oauth/authorize` endpoint after authentication. Dynamic client registration is enabled, including loopback redirect URIs required by local CLI clients such as OpenCode. MCP access tokens are Faktory-issued and resource-bound; browser sessions and Authentik tokens are not accepted as MCP bearer tokens. Disabled mode bypasses this hosted flow only for local Compose.

Client ID Metadata Documents (CIMD), dynamic client registration, and loopback redirects use separate policy switches. `FAKTORY_OAUTH_ALLOW_CIMD` defaults to `false`; DCR or loopback support does not enable CIMD. The Pulumi program explicitly enables all three policies for its hosted preview and production declarations.

When CIMD is enabled, Faktory can retrieve a client metadata document from an HTTPS URL supplied as the OAuth `client_id`. The shared hardened fetcher disables redirects, applies time and response-size bounds, pins requests to approved DNS results, and rejects non-public destinations by default. `FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS` can name a bounded comma-separated set of credential-free HTTPS root origins. Only exact listed origins can resolve to private addresses. Faktory declares no trusted private CIMD origin because this repository proves no required private metadata endpoint. These declarations and runtime controls do not prove live metadata retrieval.

Pulumi creates one 256-bit OAuth wrapping key for each configured version. The keyring names one active version and retains older versions for encrypted database records. Version identifiers use unique DNS labels. The configuration accepts from 1 through 32 versions. Pulumi stores key material as secret state and projects it only through the `faktory-oauth-wrapping-keys` Secret. A secret-derived checksum rolls the server pod after a keyring change. The runtime rejects a keyring that cannot decrypt required OAuth signing state.

Any authenticated MCP principal can retrieve, create, and edit model source under the `faktory:use` scope. The principal can inspect one technical or shaded canonical projection per call. The principal can also inspect one exact saved shaded view. The MVP defines no narrower source-author role. Source access remains MCP-only. MCP tools do not expose arbitrary object keys, Python execution arguments, renderer commands, storage credentials, GLB bytes, or storage-internal metadata. PNG bytes appear only in authorized semantic inspect results. Browser artifact URLs and browser sessions do not apply to MCP. [`protocol.md`](protocol.md) defines tool results and errors. [`storage-rendering.md`](storage-rendering.md) defines model identity and source semantics.

## Visual Renderer Boundary

The visual renderer contains Node, Playwright, Chromium, and the render harness. It contains no SPA, Faktory authentication path, Rust server, or CadQuery runtime. The service exposes unauthenticated RPC only inside the cluster. A strict NetworkPolicy admits port 8081 only from the Faktory server pod and denies all worker egress. The worker therefore relies on network isolation for caller authentication.

For each job, the worker holds the GLB in memory behind a random loopback-only harness URL. It does not persist the model. Browser interception permits only the job page, model URL, and harness asset. It blocks all external requests, and a GLB with external dependencies fails.

Chromium retains its own sandbox. The launch config enables `chromiumSandbox` and does not pass `--no-sandbox`. The worker pod alone uses outer seccomp `Unconfined`. Kubernetes `RuntimeDefault` and a narrower tested profile deny syscalls or `chroot` that Chromium needs for its sandbox. This exception does not disable the Chromium sandbox and does not apply to the Faktory server pod, which retains `RuntimeDefault`.

The worker runs as non-root UID and GID 65532. Its pod disables service-account token mounts, prevents privilege escalation, drops all capabilities, uses a read-only root filesystem, and mounts a 256 MiB memory-backed `/tmp`. The declaration starts one replica on AMD64. Native ARM64 worker behavior remains unknown and unsupported.

## Data Safety

Credentials, source text, GLB, SVG, PNG bytes, session identifiers, authorization codes, tokens, object-store internals, and raw renderer output must not enter logs, traces, metrics, protobuf errors, or browser diagnostics. Authorized MCP `model.get` results provide the only intentional source-text response. Authorized `model.inspect` and `view.inspect` results provide the only intentional PNG responses. Render failures expose only safe bounded summaries.

Browser Faro telemetry is the explicit exception to this strict server-signal contract. Its accepted collection and privacy boundary are defined in [`observability.md`](observability.md); the server exclusions above must not be used to imply that standard Faro collection is globally sanitized.
