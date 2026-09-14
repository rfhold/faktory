# Access and Authentication

## Status

Explicit disabled and production authentication paths are implemented. Compose sets exact `FAKTORY_AUTH_MODE=disabled` and contains no identity provider. Pulumi declares the production Authentik application/provider and externally supplied secrets. No Authentik resource, OAuth client, session key, credential, or route has been created externally.

## Browser Access

In production, Authentik authenticates browser users. The Faktory server owns the resulting browser session and applies it to the static SPA, gRPC-web, and HTTP geometry requests. Browser clients do not send Authentik access tokens directly to application APIs.

Production requires HTTPS for the public URL and OIDC issuer. Userinfo, query strings, and fragments remain invalid. Missing `FAKTORY_AUTH_MODE` selects production rather than disabled mode, and missing or invalid production settings fail startup.

Disabled mode is accepted only through exact `FAKTORY_AUTH_MODE=disabled`. It applies no browser middleware or interceptor to the SPA, gRPC-web, or artifact routes, and serves MCP without OAuth or bearer authentication. This mode is intended only for the Compose port published on loopback and must never be exposed beyond the local machine.

Every authenticated web user may list models, inspect render state, retrieve current successful geometry, list shared views, create or update a named view, delete a named view, and select a default view. Views are shared model state rather than per-user preferences.

## MCP Access

In production, the Faktory service is the hosted OAuth authorization server for MCP clients and uses Authentik for interactive user authentication. Dynamic client registration is enabled, including loopback redirect URIs required by local CLI clients such as OpenCode. MCP access tokens are Faktory-issued and resource-bound; browser sessions and Authentik tokens are not accepted as MCP bearer tokens. Disabled mode bypasses this hosted flow only for local Compose.

Any authenticated MCP principal may create models and edit model source. The MVP defines no narrower source-author role. Source mutation remains MCP-only. MCP tools do not expose arbitrary object keys, Python execution arguments, renderer commands, storage credentials, or geometry bytes. [`storage-rendering.md`](storage-rendering.md) defines model identity and source mutation semantics.

## Data Safety

Credentials, source text, GLB or SVG bytes, session identifiers, authorization codes, tokens, object-store internals, and raw renderer output must not appear in server logs, traces, metrics, protobuf errors, or browser-visible application diagnostics. Render failures expose only safe bounded summaries.

Browser Faro telemetry is the explicit exception to this strict server-signal contract. Its accepted collection and privacy boundary are defined in [`observability.md`](observability.md); the server exclusions above must not be used to imply that standard Faro collection is globally sanitized.
