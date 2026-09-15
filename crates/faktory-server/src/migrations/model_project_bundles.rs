//! Migration from legacy single-source revisions to project bundles.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    migrations::{put_immutable, put_immutable_json},
    model::{
        ModelRecord, RepositoryError, ViewRecord, geometry_key, preview_key,
        project::{ProjectBundle, hex_digest, project_key},
        source_key, validate_model_record, validate_view_record,
    },
    storage::{ObjectStore, PutCondition},
};

pub(super) const ID: &str = "0001-model-project-bundles";
pub(super) const DESCRIPTION: &str = "convert legacy source revisions to canonical model projects";
const ROOT: &str = "system/migrations/0001-model-project-bundles";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    objects: Vec<InventoryObject>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct InventoryObject {
    key: String,
    etag: String,
    size: usize,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelCheckpoint {
    model_id: String,
    revisions: BTreeMap<String, String>,
    backup_model_sha256: String,
    inventory_sha256: String,
    migrated_model_sha256: String,
    complete: bool,
}

pub(super) async fn run(store: &dyn ObjectStore) -> Result<(), RepositoryError> {
    let mut model_keys = store
        .list("models/")
        .await?
        .into_iter()
        .filter(|key| key.ends_with("/model.json"))
        .collect::<Vec<_>>();
    model_keys.sort();
    for key in model_keys {
        migrate_model(store, &key).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn migrate_model(store: &dyn ObjectStore, key: &str) -> Result<(), RepositoryError> {
    let model_id = key
        .strip_prefix("models/")
        .and_then(|value| value.strip_suffix("/model.json"))
        .filter(|value| !value.contains('/'))
        .ok_or(RepositoryError::Corrupt)?;
    let checkpoint_key = format!("{ROOT}/items/{model_id}.json");
    match store.get(&checkpoint_key).await {
        Ok(object) => {
            let checkpoint: ModelCheckpoint =
                serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
            return verify_checkpoint(store, key, &checkpoint).await;
        }
        Err(crate::storage::StorageError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let current = store.get(key).await?;
    let backup_key = format!("{ROOT}/backup/{model_id}/model.json");
    let original = match store.get(&backup_key).await {
        Ok(object) => object.bytes,
        Err(crate::storage::StorageError::NotFound) => {
            put_immutable(store, &backup_key, current.bytes.clone()).await?;
            current.bytes.clone()
        }
        Err(error) => return Err(error.into()),
    };
    let legacy: ModelRecord =
        serde_json::from_slice(&original).map_err(|_| RepositoryError::Corrupt)?;
    if legacy.id != model_id {
        return Err(RepositoryError::Corrupt);
    }
    validate_model_record(&legacy, model_id)?;
    let (inventory, inventory_sha256) = backup_inventory(store, model_id).await?;
    validate_legacy_views(store, model_id, &inventory).await?;
    let original_model = inventory
        .objects
        .iter()
        .find(|item| item.key == key)
        .ok_or(RepositoryError::Corrupt)?;
    if original_model.size != original.len()
        || original_model.sha256 != hex_digest(Sha256::digest(&original))
    {
        return Err(RepositoryError::Corrupt);
    }
    let mut revisions = BTreeMap::new();
    for legacy_revision in referenced_revisions(&legacy) {
        let source = backup_source(store, model_id, legacy_revision).await?;
        let project =
            ProjectBundle::single_source(&source).map_err(|_| RepositoryError::Corrupt)?;
        let revision = project.digest()?;
        put_immutable(
            store,
            &project_key(model_id, &revision),
            Bytes::from(
                project
                    .canonical_bytes()
                    .map_err(|_| RepositoryError::Corrupt)?,
            ),
        )
        .await?;
        put_immutable(store, &source_key(model_id, &revision), source).await?;
        copy_revision_objects(store, model_id, legacy_revision, &revision).await?;
        revisions.insert(legacy_revision.to_owned(), revision);
    }
    verify_successful_serving_set(store, model_id, &legacy, &revisions).await?;
    let mut migrated = legacy;
    migrated.desired_source_revision = revisions
        .get(&migrated.desired_source_revision)
        .ok_or(RepositoryError::Corrupt)?
        .clone();
    if !migrated.current_successful_source_revision.is_empty() {
        migrated.current_successful_source_revision = revisions
            .get(&migrated.current_successful_source_revision)
            .ok_or(RepositoryError::Corrupt)?
            .clone();
    }
    let migrated_bytes =
        Bytes::from(serde_json::to_vec(&migrated).map_err(|_| RepositoryError::Corrupt)?);
    if current.bytes != migrated_bytes {
        store
            .put(
                key,
                migrated_bytes.clone(),
                PutCondition::Matches(current.etag),
            )
            .await?;
    }
    let checkpoint = ModelCheckpoint {
        model_id: model_id.to_owned(),
        revisions,
        backup_model_sha256: hex_digest(Sha256::digest(&original)),
        inventory_sha256,
        migrated_model_sha256: hex_digest(Sha256::digest(&migrated_bytes)),
        complete: true,
    };
    verify_checkpoint_candidate(store, key, &checkpoint).await?;
    put_immutable_json(store, &checkpoint_key, &checkpoint).await
}

fn referenced_revisions(model: &ModelRecord) -> BTreeSet<&str> {
    let mut revisions = BTreeSet::from([model.desired_source_revision.as_str()]);
    if !model.current_successful_source_revision.is_empty() {
        revisions.insert(&model.current_successful_source_revision);
    }
    revisions
}

async fn backup_inventory(
    store: &dyn ObjectStore,
    model_id: &str,
) -> Result<(Inventory, String), RepositoryError> {
    let key = format!("{ROOT}/backup/{model_id}/inventory.json");
    match store.get(&key).await {
        Ok(object) => {
            let inventory: Inventory =
                serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
            verify_inventory_objects(store, &inventory, &format!("models/{model_id}/model.json"))
                .await?;
            return Ok((inventory, hex_digest(Sha256::digest(&object.bytes))));
        }
        Err(crate::storage::StorageError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let mut objects = Vec::new();
    for object_key in store.list(&format!("models/{model_id}/")).await? {
        let object = store.get(&object_key).await?;
        objects.push(InventoryObject {
            key: object_key,
            etag: object.etag,
            size: object.bytes.len(),
            sha256: hex_digest(Sha256::digest(&object.bytes)),
        });
    }
    objects.sort_by(|left, right| left.key.cmp(&right.key));
    let inventory = Inventory { objects };
    put_immutable_json(store, &key, &inventory).await?;
    Ok((
        inventory,
        hex_digest(Sha256::digest(&store.get(&key).await?.bytes)),
    ))
}

async fn backup_source(
    store: &dyn ObjectStore,
    model_id: &str,
    revision: &str,
) -> Result<Bytes, RepositoryError> {
    let key = format!("{ROOT}/backup/{model_id}/sources/{revision}/source.py");
    match store.get(&key).await {
        Ok(object) => {
            let legacy = store.get(&source_key(model_id, revision)).await?;
            if legacy.bytes != object.bytes {
                return Err(RepositoryError::Corrupt);
            }
            Ok(object.bytes)
        }
        Err(crate::storage::StorageError::NotFound) => {
            let source = store.get(&source_key(model_id, revision)).await?.bytes;
            put_immutable(store, &key, source.clone()).await?;
            Ok(source)
        }
        Err(error) => Err(error.into()),
    }
}

async fn verify_inventory_objects(
    store: &dyn ObjectStore,
    inventory: &Inventory,
    mutable_model_key: &str,
) -> Result<(), RepositoryError> {
    for item in inventory
        .objects
        .iter()
        .filter(|item| item.key != mutable_model_key)
    {
        let object = store.get(&item.key).await?;
        if object.etag != item.etag
            || object.bytes.len() != item.size
            || hex_digest(Sha256::digest(&object.bytes)) != item.sha256
        {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

async fn validate_legacy_views(
    store: &dyn ObjectStore,
    model_id: &str,
    inventory: &Inventory,
) -> Result<(), RepositoryError> {
    let prefix = format!("models/{model_id}/views/");
    for item in inventory
        .objects
        .iter()
        .filter(|item| item.key.starts_with(&prefix))
    {
        let view_id = item
            .key
            .strip_prefix(&prefix)
            .and_then(|suffix| suffix.strip_suffix(".json"))
            .filter(|value| !value.contains('/'))
            .ok_or(RepositoryError::Corrupt)?;
        let object = store.get(&item.key).await?;
        let view: ViewRecord =
            serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
        validate_view_record(&view, view_id)?;
    }
    Ok(())
}

async fn copy_revision_objects(
    store: &dyn ObjectStore,
    model_id: &str,
    old: &str,
    new: &str,
) -> Result<(), RepositoryError> {
    let old_prefix = format!("models/{model_id}/revisions/{old}/");
    let new_prefix = format!("models/{model_id}/revisions/{new}/");
    for old_key in store.list(&old_prefix).await? {
        let suffix = old_key
            .strip_prefix(&old_prefix)
            .ok_or(RepositoryError::Corrupt)?;
        if suffix == "source.py" {
            continue;
        }
        let source = store.get(&old_key).await?;
        let new_key = format!("{new_prefix}{suffix}");
        put_immutable(store, &new_key, source.bytes.clone()).await?;
        if store.get(&new_key).await?.bytes != source.bytes {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

async fn verify_successful_serving_set(
    store: &dyn ObjectStore,
    model_id: &str,
    legacy: &ModelRecord,
    revisions: &BTreeMap<String, String>,
) -> Result<(), RepositoryError> {
    if legacy.current_successful_source_revision.is_empty() {
        return Ok(());
    }
    let revision = revisions
        .get(&legacy.current_successful_source_revision)
        .ok_or(RepositoryError::Corrupt)?;
    verify_serving_revision(store, model_id, revision).await
}

async fn verify_serving_revision(
    store: &dyn ObjectStore,
    model_id: &str,
    revision: &str,
) -> Result<(), RepositoryError> {
    store.get(&geometry_key(model_id, revision)).await?;
    store.get(&preview_key(model_id, revision)).await?;
    Ok(())
}

async fn verify_checkpoint(
    store: &dyn ObjectStore,
    model_key: &str,
    checkpoint: &ModelCheckpoint,
) -> Result<(), RepositoryError> {
    if !checkpoint.complete {
        return Err(RepositoryError::Corrupt);
    }
    verify_checkpoint_candidate(store, model_key, checkpoint).await
}

async fn verify_checkpoint_candidate(
    store: &dyn ObjectStore,
    model_key: &str,
    checkpoint: &ModelCheckpoint,
) -> Result<(), RepositoryError> {
    let model = store.get(model_key).await?;
    let backup = store
        .get(&format!("{ROOT}/backup/{}/model.json", checkpoint.model_id))
        .await?;
    let inventory = store
        .get(&format!(
            "{ROOT}/backup/{}/inventory.json",
            checkpoint.model_id
        ))
        .await?;
    let inventory_record: Inventory =
        serde_json::from_slice(&inventory.bytes).map_err(|_| RepositoryError::Corrupt)?;
    if hex_digest(Sha256::digest(&backup.bytes)) != checkpoint.backup_model_sha256
        || hex_digest(Sha256::digest(&inventory.bytes)) != checkpoint.inventory_sha256
    {
        return Err(RepositoryError::Corrupt);
    }
    if hex_digest(Sha256::digest(&model.bytes)) != checkpoint.migrated_model_sha256 {
        return Err(RepositoryError::Corrupt);
    }
    verify_inventory_objects(store, &inventory_record, model_key).await?;
    for (legacy_revision, revision) in &checkpoint.revisions {
        let source = store
            .get(&source_key(&checkpoint.model_id, revision))
            .await?;
        let backup_source = store
            .get(&format!(
                "{ROOT}/backup/{}/sources/{legacy_revision}/source.py",
                checkpoint.model_id
            ))
            .await?;
        if source.bytes != backup_source.bytes {
            return Err(RepositoryError::Corrupt);
        }
        let object = store
            .get(&project_key(&checkpoint.model_id, revision))
            .await?;
        let project: ProjectBundle =
            serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
        if project.digest().map_err(|_| RepositoryError::Corrupt)? != *revision
            || project
                .canonical_bytes()
                .map_err(|_| RepositoryError::Corrupt)?
                != object.bytes
        {
            return Err(RepositoryError::Corrupt);
        }
    }
    let record: ModelRecord =
        serde_json::from_slice(&model.bytes).map_err(|_| RepositoryError::Corrupt)?;
    if record.id != checkpoint.model_id {
        return Err(RepositoryError::Corrupt);
    }
    if !record.current_successful_source_revision.is_empty() {
        verify_serving_revision(
            store,
            &checkpoint.model_id,
            &record.current_successful_source_revision,
        )
        .await?;
    }
    Ok(())
}
