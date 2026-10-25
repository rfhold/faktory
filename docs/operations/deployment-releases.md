# Deployment and Releases

## Declared Environments

[`infra/pulumi/index.ts`](../../infra/pulumi/index.ts) is one program parameterized by stack configuration. `Pulumi.preview.yaml` declares namespace `faktory-preview`, hostname `preview-faktory.holdenitdown.net`, 10 GiB database storage, 14-day backup retention, and unprotected data resources. `Pulumi.prod.yaml` declares namespace `faktory`, hostname `faktory.holdenitdown.net`, 20 GiB database storage, 30-day retention, and `protectData=true`.

Each stack declares one `Recreate` server replica, one `Recreate` AMD64 visual-renderer replica, PostgreSQL, Authentik integration, an HTTP route, network policies, and separate `faktory-artifacts` and `faktory-backups` ObjectBucketClaims. The route exposes only the server. The `s3Endpoint` setting applies only to server artifact storage. The separate `backupEndpoint` setting applies only to CloudNativePG Barman. Generated artifact claim configuration and credentials feed the server; generated backup claim configuration and credentials feed CloudNativePG Barman. Base backups and WAL use gzip compression. `faktory-postgres` `ScheduledBackup` requests an immediate backup on creation and then runs daily at 02:00 UTC using the six-field schedule `0 0 2 * * *`.

Optional `companionHostname` enables a distinct static host, Service, route, and no-egress NetworkPolicy; it has no default hostname.
The production stack explicitly declares `companion-faktory.holdenitdown.net`; preview declares `preview-companion-faktory.holdenitdown.net`.
Activation requires an immutable `companionImage` digest and explicit DNS, TLS, parent-origin, and cookie-scope review.
Without the hostname, Pulumi creates no companion resources and supplies no companion origin to the browser.
Internal ExternalDNS reconciles these HTTPRoute hostnames through `gateway-httproute`; the shared Gateway supports wildcard TLS for `*.holdenitdown.net`.
This internal DNS path requires no manual record; public DNS remains a separate authority.
[Companion Host Operation](companion-host.md) owns the activation prerequisites.

The server network policy preserves ingress from the `ingress` namespace on TCP 8080. Egress permits DNS, namespace-local PostgreSQL, the visual renderer on TCP 8081, and destination-unrestricted TCP 443, 4040, and 4318. The visual-renderer policy admits TCP 8081 only from the server pod and permits no egress. These are declarations rather than deployment evidence; the server's port-based external rule remains broader than destination-scoped egress.

The production data claims and PostgreSQL cluster carry Pulumi protection. Retention and protection do not prove backup completion, restore viability, bucket durability, or deployment. They do not authorize `pulumi up`, deletion, unprotection, or access to any named environment.

## Delivery Declarations

The preview pipeline in [`.tekton/faktory-preview.yaml`](../../.tekton/faktory-preview.yaml) targets `main` push/incoming events. After checkout and the pinned Gitleaks scan, five independent `service-build-v1` Tasks build server and companion-host images for Linux AMD64 and ARM64, plus an AMD64-only visual-renderer image. Each architecture is paired with its native BuildKit endpoint. Targets are `runtime`, `companion-runtime`, and `visual-renderer-runtime`; all require user `65532:65532` and the full commit SHA through `REVISION`. Only the Rust server builds receive the Secret-bound Git credential workspace. The visual renderer is the explicit single-platform exception: its Dockerfile guard, Chromium sandbox, security settings, and runtime AMD64 placement remain unchanged.

Each image uses `service-manifest-v1` to join immutable build results in AMD64/ARM64 order. The visual renderer also receives an index, with exactly one AMD64 entry. Main publishes bare full-SHA aliases:

- `faktory:<full-commit-SHA>`
- `faktory-visual-renderer:<full-commit-SHA>`
- `faktory-companion-host:<full-commit-SHA>`

The signed release pipeline in [`.tekton/faktory-release.yaml`](../../.tekton/faktory-release.yaml) targets only exact stable `vX.Y.Z` tags. One `service-promote-v1` Task resolves the three full-SHA indexes once, validates exact platforms, runtime users, and OCI revisions, and prechecks every stable destination before publishing any exact source index bytes. There are no release builds or source-manifest joins. Missing sources fail without a rebuild fallback. No application `latest` alias is published.

Neither deployment DAG runs CadQuery, visual-renderer, companion, or other functional tests. Local verifier helpers and test suites remain available; the full local quality sequence remains a required pre-push check. Security scans and shared image identity validation still gate delivery. Maintained CI tools use `general-ci:latest` with `imagePullPolicy: Always`; retained third-party tools keep their exact pins, including Gitleaks.

Both environments use `service-pulumi-up-v1` with `source-subdir=infra/pulumi`, frozen Bun installation, and direct `pulumi up --yes --skip-preview`; no standalone preview runs. Preview maps manifest results to `image`, `visualRendererImage`, and `companionImage`; production consumes the promotion Task's complete immutable image map. Authentik credentials are enabled; Grafana and OpenBao access are disabled. The optional hostname still controls companion resource creation.

These declarations do not prove pipeline success or stack deployment. All four homelab shared v1 Tasks require separate installation before consumer execution. The signed release path requires a committed remote `main`, configured Pipelines-as-Code, successful main publication for the exact release commit, and the `faktory-release-trusted-signers` Secret containing the trusted public signing key. Native BuildKit workers, registry, Git, Pulumi, Authentik, and cluster credentials and services are external prerequisites, not resources proven by this repository. No native image execution or live deployment is established by static validation.

## Signed Release Flow

1. Complete the full checks in [`../quality/testing.md`](../quality/testing.md) on the intended commit.
2. Verify the commit's `origin/main` membership and successful main publication of all three full-SHA indexes. Verify that `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json` equal `X.Y.Z`. Retain the immutable index references as publication evidence, not functional-test evidence.
3. Create an annotated, signed `vX.Y.Z` tag with a key present in the trusted signer Secret, then push that exact tag. This is an external mutation and requires repository release authority.
4. The pipeline verifies the exact tag syntax, full webhook SHA, annotated tag object, signature, trusted signer, tag-to-webhook equality, `origin/main` ancestry, and both version values.
5. Require a successful Gitleaks scan before the single image-set promotion. Resolve each bare full-SHA index once. Missing sources or registry lookup failures stop release; no rebuild fallback exists.
6. Validate both server and companion-host platform configs and the visual-renderer's single Linux AMD64 config. Every config must declare user `65532:65532` and the exact release commit as its OCI revision.
7. Preflight all three stable destinations before the first publication. Each alias must be absent through a structured `MANIFEST_UNKNOWN` 404, or equal its source digest. Conflicts, authorization failures, network errors, and unclassified errors stop promotion.
8. Recheck each destination before publishing the exact source index bytes and verify its digest afterward. Equal aliases permit idempotent retries. Registry writes lack cross-repository atomicity; an earlier equal alias can remain after a later failure. Registry-side immutable tags or exclusive writers remain necessary against concurrent external writers. Deployment requires all three verifications.
9. Supply the promotion result's complete image map to direct `pulumi up --stack prod --yes --skip-preview` through `image`, `visualRendererImage`, and `companionImage`. Pulumi rejects mutable image references.

## Availability and Rollback

The deployment has one replica and strategy `Recreate`. An update stops the prior pod before the replacement becomes available, so every preview, production, or rollback deployment has expected service downtime. There is no automatic application rollback in the pipeline.

A failed or cancelled PipelineRun does not prove rollback or the absence of external effects. Completed image publication and Pulumi changes can remain. Remote BuildKit can continue or finish after cancellation; its final status requires separate confirmation. Before an authorized retry, inspect immutable aliases and digests, remote BuildKit status, and Pulumi pending operations and resources. Live inspection, retry, and rollback each require exact target-specific authorization.

Manual rollback requires target-specific production authorization and reviewed server, visual-renderer, and enabled companion-host digests.
All selected images must remain compatible with current database, object, RPC, recipe, and companion-kit contracts.
After migration `0002-model-dependency-cutover`, older binaries fail closed on its permanent completion ledger.
From `infra/pulumi`, an authorized operator previews the exact image set before the apply:

```bash
pulumi preview --stack prod --diff \
  --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>' \
  --config 'visualRendererImage=cr.holdenitdown.net/rfhold/faktory-visual-renderer@sha256:<64-hex-digest>' \
  --config 'companionImage=cr.holdenitdown.net/rfhold/faktory-companion-host@sha256:<64-hex-digest>'
pulumi up --stack prod --yes --skip-preview \
  --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>' \
  --config 'visualRendererImage=cr.holdenitdown.net/rfhold/faktory-visual-renderer@sha256:<64-hex-digest>' \
  --config 'companionImage=cr.holdenitdown.net/rfhold/faktory-companion-host@sha256:<64-hex-digest>'
```

Record the selected registry paths and digests for the incident or change.
Do not substitute tags.
If the stack has no `companionHostname`, the companion image does not create resources.
If rollback requires database recovery, use the boundary in [`runtime-recovery.md`](runtime-recovery.md).
Image rollback does not restore PostgreSQL or artifact objects.
