//! Ordered forward-only object-store migrations.

mod model_project_bundles;

use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    model::RepositoryError,
    storage::{ObjectStore, PutCondition, StorageError},
};

const REGISTRY: &[Migration] = &[Migration {
    id: model_project_bundles::ID,
    description: model_project_bundles::DESCRIPTION,
}];

struct Migration {
    id: &'static str,
    description: &'static str,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CompletionRecord {
    id: String,
    description: String,
    complete: bool,
}

pub async fn run(store: Arc<dyn ObjectStore>) -> Result<(), RepositoryError> {
    store.ready().await?;
    validate_ledger(store.as_ref()).await?;
    for migration in REGISTRY {
        let key = ledger_key(migration.id);
        match store.get(&key).await {
            Ok(object) => {
                let record: CompletionRecord =
                    serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
                if !record.complete
                    || record.id != migration.id
                    || record.description != migration.description
                {
                    return Err(RepositoryError::Corrupt);
                }
            }
            Err(StorageError::NotFound) => {
                match migration.id {
                    model_project_bundles::ID => model_project_bundles::run(store.as_ref()).await?,
                    _ => return Err(RepositoryError::Corrupt),
                }
                put_immutable_json(
                    store.as_ref(),
                    &key,
                    &CompletionRecord {
                        id: migration.id.to_owned(),
                        description: migration.description.to_owned(),
                        complete: true,
                    },
                )
                .await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn validate_ledger(store: &dyn ObjectStore) -> Result<(), RepositoryError> {
    for key in store.list("system/migrations/").await? {
        let Some(id) = key
            .strip_prefix("system/migrations/")
            .and_then(|suffix| suffix.strip_suffix(".json"))
            .filter(|id| !id.contains('/'))
        else {
            continue;
        };
        if !REGISTRY.iter().any(|migration| migration.id == id) {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

fn ledger_key(id: &str) -> String {
    format!("system/migrations/{id}.json")
}

pub(super) async fn put_immutable(
    store: &dyn ObjectStore,
    key: &str,
    candidate: Bytes,
) -> Result<(), RepositoryError> {
    match store
        .put(key, candidate.clone(), PutCondition::Absent)
        .await
    {
        Ok(_) => Ok(()),
        Err(StorageError::Conflict) => {
            if store.get(key).await?.bytes == candidate {
                Ok(())
            } else {
                Err(RepositoryError::Conflict)
            }
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn put_immutable_json<T: Serialize + Sync>(
    store: &dyn ObjectStore,
    key: &str,
    value: &T,
) -> Result<(), RepositoryError> {
    put_immutable(
        store,
        key,
        Bytes::from(serde_json::to_vec(value).map_err(|_| RepositoryError::Corrupt)?),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            GeometryFactsRecord, GeometrySizeRecord, ModelRecord, StoredRenderState,
            TechnicalProjection, TimestampRecord, geometry_key, model_key, preview_key,
            projection_key, shaded_projection_key, source_key,
        },
        storage::{InMemoryObjectStore, ObjectStore, PutCondition},
    };
    use bytes::Bytes;
    use std::sync::Arc;

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_is_idempotent_and_maps_legacy_last_good() {
        let store = Arc::new(InMemoryObjectStore::default());
        let desired = "1".repeat(64);
        let successful = "2".repeat(64);
        for (revision, source) in [
            (&desired, b"desired".as_slice()),
            (&successful, b"good".as_slice()),
        ] {
            store
                .put(
                    &source_key("part", revision),
                    Bytes::copy_from_slice(source),
                    PutCondition::Absent,
                )
                .await
                .expect("source");
        }
        store
            .put(
                &geometry_key("part", &successful),
                Bytes::from_static(b"glb"),
                PutCondition::Absent,
            )
            .await
            .expect("geometry");
        store
            .put(
                &preview_key("part", &successful),
                Bytes::from_static(b"svg"),
                PutCondition::Absent,
            )
            .await
            .expect("preview");
        for projection in TechnicalProjection::ALL {
            store
                .put(
                    &projection_key("part", &successful, projection),
                    Bytes::from_static(b"png"),
                    PutCondition::Absent,
                )
                .await
                .expect("projection");
            store
                .put(
                    &shaded_projection_key("part", &successful, projection),
                    Bytes::from_static(b"shade"),
                    PutCondition::Absent,
                )
                .await
                .expect("shaded");
        }
        let legacy = ModelRecord {
            id: "part".to_owned(),
            name: "Part".to_owned(),
            desired_source_revision: desired.clone(),
            current_successful_source_revision: successful.clone(),
            render_state: StoredRenderState::Failed,
            render_error: "bad edit".to_owned(),
            default_view_id: String::new(),
            current_successful_facts: Some(GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            }),
            updated_at: TimestampRecord {
                seconds: 1,
                nanos: 0,
            },
        };
        let legacy_bytes = Bytes::from(serde_json::to_vec(&legacy).expect("JSON"));
        store
            .put(
                &model_key("part"),
                legacy_bytes.clone(),
                PutCondition::Absent,
            )
            .await
            .expect("model");
        run(store.clone()).await.expect("migration");
        let first_keys = store.list("").await.expect("keys");
        let migrated: ModelRecord =
            serde_json::from_slice(&store.get(&model_key("part")).await.expect("model").bytes)
                .expect("JSON");
        assert_ne!(migrated.desired_source_revision, desired);
        assert_ne!(migrated.current_successful_source_revision, successful);
        assert_ne!(
            migrated.desired_source_revision,
            migrated.current_successful_source_revision
        );
        assert_eq!(
            store
                .get(&geometry_key(
                    "part",
                    &migrated.current_successful_source_revision
                ))
                .await
                .expect("copied")
                .bytes,
            Bytes::from_static(b"glb")
        );
        assert_eq!(
            store
                .get("system/migrations/0001-model-project-bundles/backup/part/model.json")
                .await
                .expect("backup")
                .bytes,
            legacy_bytes
        );
        assert!(store.get(&source_key("part", &desired)).await.is_ok());
        for key in [
            "system/migrations/0001-model-project-bundles.json",
            "system/migrations/0001-model-project-bundles/items/part.json",
        ] {
            let object = store.get(key).await.expect("migration state");
            store
                .delete(key, &object.etag)
                .await
                .expect("simulate crash boundary");
        }
        run(store.clone()).await.expect("resume partial migration");
        assert_eq!(store.list("").await.expect("keys"), first_keys);
        run(store.clone()).await.expect("rerun");
        assert_eq!(store.list("").await.expect("keys"), first_keys);
        let rerun: ModelRecord =
            serde_json::from_slice(&store.get(&model_key("part")).await.expect("model").bytes)
                .expect("JSON");
        assert_eq!(rerun, migrated);
    }

    #[tokio::test]
    async fn unknown_completed_migration_fails_closed() {
        let store = Arc::new(InMemoryObjectStore::default());
        store
            .put(
                "system/migrations/9999-unknown.json",
                Bytes::from_static(b"{}"),
                PutCondition::Absent,
            )
            .await
            .expect("marker");
        assert_eq!(run(store).await, Err(RepositoryError::Corrupt));
    }
}
