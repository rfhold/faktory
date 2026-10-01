# Deployment and Releases

## Declared Environments

[`infra/pulumi/index.ts`](../../infra/pulumi/index.ts) is one program parameterized by stack configuration. `Pulumi.preview.yaml` declares namespace `faktory-preview`, hostname `preview-faktory.holdenitdown.net`, 10 GiB database storage, 14-day backup retention, and unprotected data resources. `Pulumi.prod.yaml` declares namespace `faktory`, hostname `faktory.holdenitdown.net`, 20 GiB database storage, 30-day retention, and `protectData=true`.

Each stack declares one `Recreate` server replica, one `Recreate` AMD64 visual-renderer replica, PostgreSQL, Authentik integration, an HTTP route, network policies, and separate `faktory-artifacts` and `faktory-backups` ObjectBucketClaims. The route exposes only the server. The `s3Endpoint` setting applies only to server artifact storage. The separate `backupEndpoint` setting applies only to CloudNativePG Barman. Generated artifact claim configuration and credentials feed the server; generated backup claim configuration and credentials feed CloudNativePG Barman. Base backups and WAL use gzip compression. `faktory-postgres` `ScheduledBackup` requests an immediate backup on creation and then runs daily at 02:00 UTC using the six-field schedule `0 0 2 * * *`.

The server network policy preserves ingress from the `ingress` namespace on TCP 8080. Egress permits DNS, namespace-local PostgreSQL, the visual renderer on TCP 8081, and destination-unrestricted TCP 443, 4040, and 4318. The visual-renderer policy admits TCP 8081 only from the server pod and permits no egress. These are declarations rather than deployment evidence; the server's port-based external rule remains broader than destination-scoped egress.

The production data claims and PostgreSQL cluster carry Pulumi protection. Retention and protection do not prove backup completion, restore viability, bucket durability, or deployment. They do not authorize `pulumi up`, deletion, unprotection, or access to any named environment.

## Delivery Declarations

The preview pipeline in [`.tekton/faktory-preview.yaml`](../../.tekton/faktory-preview.yaml) targets `main`. It builds Linux AMD64 and ARM64 server images. Matching native tasks run each packaged CadQuery renderer and validate its multipart artifacts and GLB before manifest publication. The signed release pipeline in [`.tekton/faktory-release.yaml`](../../.tekton/faktory-release.yaml) targets only exact stable tags of the form `vX.Y.Z`. Both declarations run a pinned Gitleaks scan. Release promotes existing preview digests; it does not rebuild images, repeat native functional checks, or assemble a new manifest.

Preview also builds a separate AMD64 visual-renderer image. A native AMD64 task starts that image with the declared hardening controls and Chromium sandbox. It renders a colored GLB and validates the `three-v2` response, seven names, PNG dimensions, MIME types, and visible color. Preview resolves both images to immutable digests. Release resolves `faktory:preview-<full-commit-SHA>` and `faktory-visual-renderer:preview-<full-commit-SHA>-amd64`, validates their platform, runtime user, and OCI revision metadata, and copies those exact digests under the stable version tag. Both deploy tasks pass the original server and visual-renderer digests to Pulumi. The full local quality sequence remains a required pre-push check and does not run inside these pipelines.

These declarations do not prove pipeline success or stack deployment. The signed release path requires a committed remote `main`, a configured Pipeline-as-Code environment, a successful preview run for the exact release commit, and the `faktory-release-trusted-signers` Secret containing the trusted public signing key. Registry, Git, Pulumi, Authentik, and cluster credentials and services are external prerequisites, not resources proven by this repository. BuildKit and native verification nodes are preview prerequisites, not release prerequisites.

## Signed Release Flow

1. Complete the full checks in [`../quality/testing.md`](../quality/testing.md) on the intended commit.
2. Ensure the commit is present in `origin/main`, has a successful preview run including both native server checks and the AMD64 visual-renderer check, and that `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json` both equal `X.Y.Z`. Retain the successful run's two digest references as release evidence. Preview tag existence and OCI metadata alone do not attest functional success; the pipeline relies on the existing preview publication contract and operator review of the run.
3. Create an annotated, signed `vX.Y.Z` tag with a key present in the trusted signer Secret, then push that exact tag. This is an external mutation and requires repository release authority.
4. The pipeline verifies the exact tag syntax, full webhook SHA, annotated tag object, signature, trusted signer, tag-to-webhook equality, `origin/main` ancestry, and both version values.
5. Gitleaks must pass before promotion. Resolve both existing preview aliases to full SHA-256 digests. Missing sources or registry lookup failures stop release; there is no rebuild fallback.
6. Before any copy, validate the server manifest's AMD64 and ARM64 entries and each Linux platform's config, plus the worker's Linux AMD64 config. All configs must declare user `65532:65532` and OCI revision equal to the release commit.
7. Preflight both stable destinations before copying either image. Each alias must be absent with an explicit registry `MANIFEST_UNKNOWN` or `NAME_UNKNOWN` response, or already equal to its source digest. Conflicts, authorization failures, network errors, and unclassified lookup errors stop promotion rather than being treated as absence.
8. Recheck each destination immediately before its copy, copy from the immutable source reference only if absent, and verify both stable `vX.Y.Z` aliases equal the original preview digests. Matching aliases make retries idempotent. No `latest` alias is published. Registry writes are not atomic across repositories; if a later copy fails, an earlier matching alias can remain, but deployment does not proceed until both verify.
9. Both original immutable digests are supplied to `pulumi preview --stack prod` and `pulumi up --stack prod` through `image` and `visualRendererImage`. Pulumi policy rejects mutable image references.

## Availability and Rollback

The deployment has one replica and strategy `Recreate`. An update stops the prior pod before the replacement becomes available, so every preview, production, or rollback deployment has expected service downtime. There is no automatic application rollback in the pipeline.

Manual rollback requires target-specific production authorization and previously published, reviewed server and visual-renderer digests. Both images must remain compatible with current database, object, RPC, and recipe state. A server image that predates a completed object-store migration is not compatible; after `0002-model-dependency-cutover`, older binaries fail closed on its permanent completion ledger. From `infra/pulumi`, an authorized operator previews the exact pair before applying it:

```bash
pulumi preview --stack prod --diff \
  --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>' \
  --config 'visualRendererImage=cr.holdenitdown.net/rfhold/faktory-visual-renderer@sha256:<64-hex-digest>'
pulumi up --stack prod --yes --skip-preview \
  --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>' \
  --config 'visualRendererImage=cr.holdenitdown.net/rfhold/faktory-visual-renderer@sha256:<64-hex-digest>'
```

Record both registry paths and digests for the incident or change. Do not substitute tags. If rollback depends on database recovery, stop and use the recovery boundary in [`runtime-recovery.md`](runtime-recovery.md); image rollback does not restore PostgreSQL or artifact objects.
