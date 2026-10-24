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

The preview pipeline in [`.tekton/faktory-preview.yaml`](../../.tekton/faktory-preview.yaml) targets `main`. It builds Linux AMD64 and ARM64 server images. Matching native tasks run each packaged CadQuery renderer and validate its multipart artifacts and GLB before manifest publication. The signed release pipeline in [`.tekton/faktory-release.yaml`](../../.tekton/faktory-release.yaml) targets only exact stable tags of the form `vX.Y.Z`. Both declarations run a pinned Gitleaks scan. Maintained CI tool images use `latest` with `imagePullPolicy: Always`; the pipelines select `cr.holdenitdown.net/rfhold/general-ci:latest`. Third-party tool images retain their exact version and digest pins. Application build inputs, runtime-check image parameters, and exact tested release digests remain unchanged.
Release promotes selected preview digests without image builds, repeated CadQuery or visual-renderer checks, or a new manifest.
It performs a fresh packaged companion-host smoke check on the digest it selects.

Preview also builds separate AMD64 visual-renderer and companion-host images.
A native AMD64 task exercises the visual renderer with the declared hardening controls and Chromium sandbox.
It validates the colored GLB's `three-v2` response, seven names, PNG dimensions, MIME types, and visible color.
BuildKit's `containerimage.digest` metadata supplies the companion build result as an immutable image reference.
A companion task exercises that exact reference, not the mutable preview alias.
The next task checks that digest's runtime user and OCI revision before deployment consumes the same reference.
Preview resolves all three images to immutable digests.
Release resolves these aliases for the exact release commit:

- `faktory:preview-<full-commit-SHA>`
- `faktory-visual-renderer:preview-<full-commit-SHA>-amd64`
- `faktory-companion-host:preview-<full-commit-SHA>-amd64`

Release validates platform, runtime user, and OCI revision metadata before it copies the exact digests under the stable version tag.
The companion selection task resolves its preview alias once.
Release runs a fresh smoke check on that selected digest, then promotes and deploys that same digest.
This check establishes selected-digest smoke evidence; it does not prove equality with a prior preview run's digest.
Both deploy tasks pass their selected digests through `image`, `visualRendererImage`, and `companionImage`.
The optional hostname still controls companion resource creation.
The full local quality sequence remains a required pre-push check and does not run inside these pipelines.

These declarations do not prove pipeline success or stack deployment. The signed release path requires a committed remote `main`, a configured Pipeline-as-Code environment, a successful preview run for the exact release commit, and the `faktory-release-trusted-signers` Secret containing the trusted public signing key. Registry, Git, Pulumi, Authentik, and cluster credentials and services are external prerequisites, not resources proven by this repository.
BuildKit and native renderer verification nodes remain preview prerequisites.
Release also requires capacity to execute the selected AMD64 companion image's fresh smoke check.

## Signed Release Flow

1. Complete the full checks in [`../quality/testing.md`](../quality/testing.md) on the intended commit.
2. Verify the commit's `origin/main` membership and successful preview run, including both native server checks and both AMD64 service checks. Verify that `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json` equal `X.Y.Z`. Retain preview digest references as preview evidence. Retain the release-selected companion digest and its fresh smoke result as separate release evidence.
3. Create an annotated, signed `vX.Y.Z` tag with a key present in the trusted signer Secret, then push that exact tag. This is an external mutation and requires repository release authority.
4. The pipeline verifies the exact tag syntax, full webhook SHA, annotated tag object, signature, trusted signer, tag-to-webhook equality, `origin/main` ancestry, and both version values.
5. Require a successful Gitleaks scan before promotion. Resolve the companion preview alias once to an immutable digest. Require its fresh packaged smoke check. Preserve that tested reference through promotion. Resolve the main and visual-renderer aliases to full SHA-256 digests. Missing sources or registry lookup failures stop release; no rebuild fallback exists.
6. Before any copy, validate both server platform configs and the visual-renderer and companion-host Linux AMD64 configs. Every config must declare user `65532:65532` and the exact release commit as its OCI revision.
7. Preflight all three stable destinations before the first copy. Each alias must be absent through explicit `MANIFEST_UNKNOWN` or `NAME_UNKNOWN`, or equal its source digest. Conflicts, authorization failures, network errors, and unclassified errors stop promotion.
8. Recheck each destination before its copy. If absent, copy the immutable source reference. Verify all three stable `vX.Y.Z` aliases against their selected digests. Equal aliases permit idempotent retries. The pipeline publishes no `latest` alias. Registry writes lack cross-repository atomicity; an earlier equal alias can remain after a later failure. Deployment requires all three verifications.
9. Supply the selected digests to `pulumi preview --stack prod` and `pulumi up --stack prod` through `image`, `visualRendererImage`, and `companionImage`. Preserve the smoke-tested companion digest exactly. Pulumi rejects mutable image references.

## Availability and Rollback

The deployment has one replica and strategy `Recreate`. An update stops the prior pod before the replacement becomes available, so every preview, production, or rollback deployment has expected service downtime. There is no automatic application rollback in the pipeline.

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
