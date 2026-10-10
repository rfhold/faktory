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

Preview builds the main image, AMD64 visual-renderer image, and AMD64 companion-host image.
BuildKit metadata supplies the companion image digest directly to the build task result.
Preview exercises that exact digest, checks its revision and runtime user, and supplies the same digest to deployment.
Release selects the companion preview alias once and performs a fresh packaged smoke check on the selected digest.
It promotes and deploys that exact tested digest; it does not infer the digest from prior preview-run evidence.
Release promotes all three selected image digests and does not rebuild environment-specific bytes.
Preflight all three stable destinations before the first copy.
If any lookup, identity, platform, or destination check fails, stop promotion.
Verify all three stable aliases before deployment.
[Deployment and Releases](deployment-releases.md) owns the full release and rollback procedure.

Before companion activation, review these prerequisites:

- Exact parent and companion origins, distinct hostnames, DNS, TLS, and cookie scopes.
- Immutable `image`, `visualRendererImage`, and `companionImage` inputs.
- Host route, probes, no egress, runtime-config access, and security headers.
- Opaque frame sandbox, credentialless access, no referrer, and the exact message allowlist.
- Authorized browser-session retrieval, no generic source route, and no private content at host HTTP endpoints.

Run the local companion checks in [Testing](../quality/testing.md#companion-checks).
Production browser-session, DNS, TLS, ingress, and cookie checks require separate target-specific authorization.
