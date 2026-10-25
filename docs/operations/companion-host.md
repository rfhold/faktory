# Companion Host Operation

[The companion specification](../architecture/spec/companion.md) owns the security and browser contracts.
This guide describes configuration and declaration checks, not a live deployment record.

## Runtime Configuration

Set `FAKTORY_COMPANION_ORIGIN` to the host's exact distinct origin, or leave it unset to disable companion frames.
Use HTTPS for hosted environments.
Explicit disabled authentication also permits canonical private-network HTTP addresses, including Compose's `172.16.1.40`.
Production rejects private non-loopback HTTP origins.
The authenticated server returns the origin through no-store `GET /runtime-config`.
Do not bake an environment hostname into the SPA image.

Set `COMPANION_PARENT_ORIGINS` on the static host to exact permitted parent origins separated by spaces.
Do not include trailing slashes, paths, credentials, wildcard origins, queries, or fragments.
The host listens on `8082` by default and serves only `/frame`, `/health`, and `/ready`.
It needs no project-store credentials, browser session keys, MCP tokens, or outbound network access.

## Origin and Cookie Isolation

Use a separate cookie domain for the companion host wherever practical.
Sibling subdomains do not isolate cookies with a broad shared `Domain` attribute.
Confirm the application and identity provider cookie scopes before activation.
Keep cookies host-only when the host shares a registrable domain.
The iframe's opaque origin and credentialless attribute do not replace that deployment review.

Local Compose uses parent `http://172.16.1.40:8080` and companion `http://172.16.1.40:8082`.
Different ports separate origins, not cookie domains.
This local topology uses explicit disabled authentication on a trusted private network only.
Restrict both public local ports with the host firewall.

## Optional Infrastructure

Pulumi's optional `companionHostname` has no default hostname.
The production stack declares `companion-faktory.holdenitdown.net`; the preview stack declares `preview-companion-faktory.holdenitdown.net`.
Without that setting, the stack creates no companion Deployment, Service, route, or NetworkPolicy.
When enabled, `companionImage` must contain an immutable SHA-256 digest.
The hostname must differ from the application hostname.
The shared Gateway's wildcard TLS configuration supports these single-label hostnames under `*.holdenitdown.net`.
Internal ExternalDNS watches `gateway-httproute` and reconciles the declared HTTPRoute hostnames through its internal DNS provider.
The Pulumi program declares the hostnames and routes, not separate DNS records.
Manual internal DNS records are not a prerequisite for this supported reconciliation path.
Public DNS uses a separate authority; internal reconciliation does not establish public records or public reachability.
Operators retain responsibility for DNS, TLS, and cookie-scope prerequisites in other environments.

The declaration uses a separate static host, non-root UID/GID `65532`, a read-only root, dropped capabilities, and no service-account token.
Its NetworkPolicy admits ingress traffic on `8082` from the ingress namespace and denies all egress.
The host needs no visual-worker seccomp exception.
The server receives the companion origin through runtime environment configuration.
The companion host receives only the exact application parent origin.

## Delivery and Verification

Preview independently builds Linux AMD64 and ARM64 server and companion-host images, plus the explicit AMD64-only visual renderer, with `service-build-v1`.
The companion builds receive no Git credential workspace.
`service-manifest-v1` verifies each immutable companion build result's platform, revision, and runtime user, then publishes the dual-platform index at the bare full-SHA alias.
The visual renderer receives a single-platform index; the server receives a dual-platform index.
Neither deployment DAG runs packaged functional smoke tests; local helpers remain available for separate checks.
One release `service-promote-v1` Task resolves all three full-SHA indexes, validates their identities, and prechecks all stable destinations before any publication.
It promotes exact index bytes without rebuilding environment-specific images and verifies all stable aliases before deployment.
Both environments pass a complete immutable image map to direct `service-pulumi-up-v1` apply, without a standalone preview.
If any lookup, identity, platform, or destination check fails, stop promotion.
[Deployment and Releases](deployment-releases.md) owns the full release and rollback procedure.

Before companion activation, review these prerequisites:

- Exact parent and companion origins, distinct hostnames, DNS, TLS, and cookie scopes.
- Immutable `image`, `visualRendererImage`, and `companionImage` inputs.
- Host route, probes, no egress, runtime-config access, and security headers.
- Opaque frame sandbox, credentialless access, no referrer, and the exact message allowlist.
- Authorized browser-session retrieval, no generic source route, and no private content at host HTTP endpoints.

Run the local companion checks in [Testing](../quality/testing.md#companion-checks).
Production browser-session, DNS, TLS, ingress, and cookie checks require separate target-specific authorization.
