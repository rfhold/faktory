# OAuth Wrapping-Key Rotation And Recovery

## Purpose

Use this runbook for the Pulumi-managed Faktory OAuth wrapping-key ring. PostgreSQL stores encrypted OAuth signing records. Pulumi state and the Kubernetes Secret hold the separate wrapping keys.

This procedure does not prove a live rotation, backup, restore, or deployment.

## Preconditions And Authorization

- Obtain target-specific authorization for every preview, deployment, backup, Secret export, database query, and protection change.
- Confirm the exact Pulumi stack and Kubernetes context.
- Take a CloudNativePG backup through the approved process.
- Verify that the approved process can retrieve the selected backup.
- Export the `faktory-oauth-wrapping-keys` Secret through an approved encrypted backup process.
- Record only the active version identifier and retained version identifiers.
- Keep decoded key material out of the repository, shell history, CI logs, and Pulumi outputs.

## Staged Rotation

### 1. Add The New Version

1. Add a new DNS-label identifier to `faktory:oauthWrappingKeyVersions`.
2. Retain every current version in its current order.
3. Keep `faktory:oauthActiveWrappingKeyVersion` set to the current active version.
4. Run an authorized `pulumi preview` for the target stack.
5. Confirm that the preview creates one new `RandomBytes` resource.
6. Confirm that the preview keeps every current wrapping-key resource.
7. Apply the reviewed change under separate target authorization.
8. Verify pod readiness and OAuth issuance before the next stage.

Stop if Pulumi proposes replacement or deletion of a retained key resource.

### 2. Activate The New Version

1. Set `faktory:oauthActiveWrappingKeyVersion` to the new identifier.
2. Keep old and new identifiers in `faktory:oauthWrappingKeyVersions`.
3. Preview the change and confirm that Pulumi retains every key resource.
4. Apply the reviewed change under separate target authorization.
5. Verify pod readiness, token issuance, token refresh, and JWKS publication.
6. Verify through an approved database-safe check that required signing records remain decryptable.

### 3. Remove An Old Version

1. Confirm that no encrypted database record references the old wrapping-key identifier.
2. Take a new paired database and encrypted Secret backup.
3. Obtain explicit authorization to remove protection from the old `RandomBytes` resource.
4. Remove protection only from that exact old resource through the approved Pulumi process.
5. Remove the old identifier from `faktory:oauthWrappingKeyVersions`.
6. Preview the change and confirm that Pulumi deletes only the intended old resource.
7. Apply the reviewed change under separate target authorization.
8. Verify pod readiness and OAuth behavior again.

Never add and remove versions in one rollout. Never remove a version while any database record references it.

## Rollback

Restore a keyring that contains every version from before and during the failed rotation. Keep both keys if database records can reference either version. Change only the active identifier when that action restores prior behavior.

## Recovery

1. Restore the selected PostgreSQL recovery point through an authorized process.
2. Restore the encrypted Secret backup that contains every referenced wrapping-key version.
3. Preserve the exact key bytes for each restored version identifier.
4. Reconcile Pulumi state with those exact resources before the server starts.
5. Start one server replica.
6. Verify migrations, OAuth readiness, token issuance, refresh, and JWKS publication.

New random values cannot decrypt records from the restored database. An image rollback does not restore the database or wrapping keys.

## Stop Conditions

- Stop if the database reference check lacks conclusive evidence.
- Stop if a preview replaces or deletes an unintended key resource.
- Stop if the Secret backup omits a referenced version.
- Stop if key material appears in logs, command history, or repository files.

## References

- [`../architecture/access-authentication.md`](../architecture/access-authentication.md)
- [`runtime-recovery.md`](runtime-recovery.md)
- [`deployment-releases.md`](deployment-releases.md)
