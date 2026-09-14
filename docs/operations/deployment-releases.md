# Deployment and Releases

## Declared Environments

[`infra/pulumi/index.ts`](../../infra/pulumi/index.ts) is one program parameterized by stack configuration. `Pulumi.preview.yaml` declares namespace `faktory-preview`, hostname `preview-faktory.holdenitdown.net`, 10 GiB database storage, 14-day backup retention, and unprotected data resources. `Pulumi.prod.yaml` declares namespace `faktory`, hostname `faktory.holdenitdown.net`, 20 GiB database storage, 30-day retention, and `protectData=true`.

Each stack declares one `Recreate` server replica, PostgreSQL, Authentik integration, an HTTP route, network policy, and separate `faktory-artifacts` and `faktory-backups` ObjectBucketClaims. The `s3Endpoint` setting applies only to server artifact storage. The separate `backupEndpoint` setting applies only to CloudNativePG Barman. Generated artifact claim configuration and credentials feed the server; generated backup claim configuration and credentials feed CloudNativePG Barman. Base backups and WAL use gzip compression. `faktory-postgres` `ScheduledBackup` requests an immediate backup on creation and then runs daily at 02:00 UTC using the six-field schedule `0 0 2 * * *`.

The server network policy preserves ingress from the `ingress` namespace on TCP 8080. Egress permits DNS to the `kube-system` namespace on TCP and UDP 53, namespace-local PostgreSQL pods selected by `cnpg.io/cluster=faktory-postgres` on TCP 5432, and destination-unrestricted TCP 443, 4040, and 4318. These are declarations rather than deployment evidence; the port-based external rule is broader than destination-scoped egress.

The production data claims and PostgreSQL cluster carry Pulumi protection. Retention and protection do not prove backup completion, restore viability, bucket durability, or deployment. They do not authorize `pulumi up`, deletion, unprotection, or access to any named environment.

## Delivery Declarations

The preview pipeline in [`.tekton/faktory-preview.yaml`](../../.tekton/faktory-preview.yaml) targets `main`. The signed release pipeline in [`.tekton/faktory-release.yaml`](../../.tekton/faktory-release.yaml) targets only exact stable tags of the form `vX.Y.Z`. Both declarations run a pinned Gitleaks scan and build Linux AMD64 and ARM64 images. Before manifest publication, matching-architecture tasks run each built runtime image as packaged, render the packaged box example through CadQuery, and validate the GLB magic, version, and declared byte size. Only then do the pipelines create a multi-architecture manifest, resolve it to an immutable digest, and pass that digest to Pulumi. The full quality sequence remains a required local pre-push check and is not repeated by these delivery pipelines.

No pipeline has run and neither stack is deployed. These files cannot function as a signed release path until the repository has a committed remote `main`, the Pipeline-as-Code environment is configured, and the `faktory-release-trusted-signers` Secret contains the trusted public signing key. Registry, BuildKit, Git, Pulumi, Authentik, and cluster credentials and services are also external prerequisites, not resources proven by this repository.

## Signed Release Flow

1. Complete the full checks in [`../quality/testing.md`](../quality/testing.md) on the intended commit.
2. Ensure the commit is present in `origin/main` and that `[workspace.package].version` in `Cargo.toml` and `version` in `web/package.json` both equal `X.Y.Z`.
3. Create an annotated, signed `vX.Y.Z` tag with a key present in the trusted signer Secret, then push that exact tag. This is an external mutation and requires repository release authority.
4. The pipeline verifies the exact tag syntax, full webhook SHA, annotated tag object, signature, trusted signer, tag-to-webhook equality, `origin/main` ancestry, and both version values.
5. Gitleaks must pass before the pipeline builds architecture-specific images. Native AMD64 and ARM64 tasks must each run the matching image's packaged CadQuery renderer and validate its generated GLB before manifest publication.
6. The pipeline publishes and verifies a multi-architecture commit manifest, including each image's non-root identity and revision label, then promotes its digest to stable tag `vX.Y.Z`. Promotion is idempotent only when an existing stable tag resolves to the same digest; it fails rather than moving that tag. No `latest` alias is published.
7. The immutable digest, not the stable tag, is supplied to `pulumi preview --stack prod` and then `pulumi up --stack prod`. Pulumi policy rejects mutable image references.

## Availability and Rollback

The deployment has one replica and strategy `Recreate`. An update stops the prior pod before the replacement becomes available, so every preview, production, or rollback deployment has expected service downtime. There is no automatic application rollback in the pipeline.

Manual rollback requires target-specific production authorization and a previously published, reviewed immutable digest compatible with current database and object state. From `infra/pulumi`, an authorized operator previews the exact prior digest before applying it:

```bash
pulumi preview --stack prod --diff --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>'
pulumi up --stack prod --yes --skip-preview --config 'image=cr.holdenitdown.net/rfhold/faktory@sha256:<64-hex-digest>'
```

Record the actual registry path and digest selected for the incident or change. Do not substitute a tag. If rollback depends on database recovery, stop and use the recovery boundary in [`runtime-recovery.md`](runtime-recovery.md); image rollback does not restore PostgreSQL or artifact objects.
