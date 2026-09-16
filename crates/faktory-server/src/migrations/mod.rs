//! Ordered forward-only object-store migrations.

mod model_dependency_cutover;
mod model_project_bundles;

use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    model::RepositoryError,
    storage::{ObjectStore, PutCondition, StorageError},
};

const REGISTRY: &[Migration] = &[
    Migration {
        id: model_project_bundles::ID,
        description: model_project_bundles::DESCRIPTION,
    },
    Migration {
        id: model_dependency_cutover::ID,
        description: model_dependency_cutover::DESCRIPTION,
    },
];

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
                    model_dependency_cutover::ID => {
                        model_dependency_cutover::run(store.as_ref()).await?;
                    }
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
        storage::{InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject},
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Debug, Default)]
    struct InterruptDeleteStore {
        inner: InMemoryObjectStore,
        interrupt_next_delete: AtomicBool,
    }

    #[async_trait]
    impl ObjectStore for InterruptDeleteStore {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            self.inner.get(key).await
        }

        async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
            self.inner.list(prefix).await
        }

        async fn put(
            &self,
            key: &str,
            bytes: Bytes,
            condition: PutCondition,
        ) -> Result<String, StorageError> {
            self.inner.put(key, bytes, condition).await
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            if self.interrupt_next_delete.swap(false, Ordering::SeqCst) {
                return Err(StorageError::Unavailable);
            }
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    #[derive(Debug, Default)]
    struct CutoverMatrixStore {
        inner: InMemoryObjectStore,
        list_calls: AtomicUsize,
        fail_list_call: AtomicUsize,
        fail_delete_key: Mutex<Option<String>>,
        fail_put_key: Mutex<Option<String>>,
        paged_prefix: Mutex<Option<String>>,
    }

    impl CutoverMatrixStore {
        fn fail_delete_once(&self, key: &str) {
            *self.fail_delete_key.lock().expect("delete failpoint") = Some(key.to_owned());
        }

        fn fail_put_once(&self, key: &str) {
            *self.fail_put_key.lock().expect("put failpoint") = Some(key.to_owned());
        }

        fn fail_list_once(&self, call: usize) {
            self.list_calls.store(0, Ordering::SeqCst);
            self.fail_list_call.store(call, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl ObjectStore for CutoverMatrixStore {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            self.inner.get(key).await
        }

        async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
            let call = self.list_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_list_call.load(Ordering::SeqCst) == call {
                self.fail_list_call.store(0, Ordering::SeqCst);
                return Err(StorageError::Unavailable);
            }
            let mut keys = self.inner.list(prefix).await?;
            if self.paged_prefix.lock().expect("page failpoint").as_deref() == Some(prefix)
                && keys.len() > 1
            {
                keys.truncate(1);
            }
            Ok(keys)
        }

        async fn put(
            &self,
            key: &str,
            bytes: Bytes,
            condition: PutCondition,
        ) -> Result<String, StorageError> {
            let should_fail = {
                let mut fail = self.fail_put_key.lock().expect("put failpoint");
                let matches = fail.as_deref() == Some(key);
                if matches {
                    fail.take();
                }
                matches
            };
            if should_fail {
                return Err(StorageError::Unavailable);
            }
            self.inner.put(key, bytes, condition).await
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            let should_fail = {
                let mut fail = self.fail_delete_key.lock().expect("delete failpoint");
                let matches = fail.as_deref() == Some(key);
                if matches {
                    fail.take();
                }
                matches
            };
            if should_fail {
                return Err(StorageError::Unavailable);
            }
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    async fn pending_cutover_store() -> Arc<CutoverMatrixStore> {
        let store = Arc::new(CutoverMatrixStore::default());
        run(store.clone()).await.expect("seed migration ledgers");
        let key = ledger_key(model_dependency_cutover::ID);
        let ledger = store.get(&key).await.expect("cutover ledger");
        store
            .inner
            .delete(&key, &ledger.etag)
            .await
            .expect("reset cutover ledger");
        store.list_calls.store(0, Ordering::SeqCst);
        store
    }

    async fn put_cutover_fixture(store: &CutoverMatrixStore, key: &str) {
        store
            .inner
            .put(key, Bytes::from_static(b"fixture"), PutCondition::Absent)
            .await
            .expect("cutover fixture");
    }

    async fn assert_cutover_ledger_absent(store: &CutoverMatrixStore) {
        assert_eq!(
            store.get(&ledger_key(model_dependency_cutover::ID)).await,
            Err(StorageError::NotFound)
        );
    }

    async fn restart_cutover(store: Arc<CutoverMatrixStore>) {
        run(store.clone()).await.expect("restart cutover");
        for prefix in model_dependency_cutover::PREFIXES {
            assert!(
                store
                    .list(prefix)
                    .await
                    .expect("verified prefix")
                    .is_empty()
            );
        }
        assert!(
            store
                .get(&ledger_key(model_dependency_cutover::ID))
                .await
                .is_ok()
        );
    }

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
            current_successful_outputs: Vec::new(),
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
        model_project_bundles::run(store.as_ref())
            .await
            .expect("migration");
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
        for key in ["system/migrations/0001-model-project-bundles/items/part.json"] {
            let object = store.get(key).await.expect("migration state");
            store
                .delete(key, &object.etag)
                .await
                .expect("simulate crash boundary");
        }
        model_project_bundles::run(store.as_ref())
            .await
            .expect("resume partial migration");
        assert_eq!(store.list("").await.expect("keys"), first_keys);
        model_project_bundles::run(store.as_ref())
            .await
            .expect("rerun");
        assert_eq!(store.list("").await.expect("keys"), first_keys);
        let rerun: ModelRecord =
            serde_json::from_slice(&store.get(&model_key("part")).await.expect("model").bytes)
                .expect("JSON");
        assert_eq!(rerun, migrated);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_accepts_legacy_success_without_projection_artifacts() {
        let store = Arc::new(InMemoryObjectStore::default());
        let legacy_revision = "3".repeat(64);
        for (key, bytes) in [
            (
                source_key("legacy", &legacy_revision),
                b"part = 1".as_slice(),
            ),
            (
                geometry_key("legacy", &legacy_revision),
                b"legacy-glb".as_slice(),
            ),
            (
                preview_key("legacy", &legacy_revision),
                b"legacy-svg".as_slice(),
            ),
        ] {
            store
                .put(&key, Bytes::copy_from_slice(bytes), PutCondition::Absent)
                .await
                .expect("legacy object");
        }
        let legacy = ModelRecord {
            id: "legacy".to_owned(),
            name: "Legacy".to_owned(),
            desired_source_revision: legacy_revision.clone(),
            current_successful_source_revision: legacy_revision.clone(),
            render_state: StoredRenderState::Ready,
            render_error: String::new(),
            default_view_id: String::new(),
            current_successful_facts: Some(GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            }),
            current_successful_outputs: Vec::new(),
            updated_at: TimestampRecord {
                seconds: 1,
                nanos: 0,
            },
        };
        store
            .put(
                &model_key("legacy"),
                Bytes::from(serde_json::to_vec(&legacy).expect("JSON")),
                PutCondition::Absent,
            )
            .await
            .expect("legacy model");

        model_project_bundles::run(store.as_ref())
            .await
            .expect("migration");
        let migrated: ModelRecord = serde_json::from_slice(
            &store
                .get(&model_key("legacy"))
                .await
                .expect("migrated model")
                .bytes,
        )
        .expect("JSON");
        assert_ne!(migrated.desired_source_revision, legacy_revision);
        assert_eq!(
            migrated.desired_source_revision,
            migrated.current_successful_source_revision
        );
        assert_eq!(
            store
                .get(&source_key("legacy", &migrated.desired_source_revision))
                .await
                .expect("copied source")
                .bytes,
            Bytes::from_static(b"part = 1")
        );
        assert_eq!(
            store
                .get(&geometry_key("legacy", &migrated.desired_source_revision))
                .await
                .expect("copied geometry")
                .bytes,
            Bytes::from_static(b"legacy-glb")
        );
        assert_eq!(
            store
                .get(&preview_key("legacy", &migrated.desired_source_revision))
                .await
                .expect("copied preview")
                .bytes,
            Bytes::from_static(b"legacy-svg")
        );
        let first_keys = store.list("").await.expect("keys");
        model_project_bundles::run(store.as_ref())
            .await
            .expect("idempotent rerun");
        assert_eq!(store.list("").await.expect("keys"), first_keys);
    }

    #[tokio::test]
    async fn model_dependency_cutover_deletes_legacy_product_data_and_preserves_ledgers() {
        let store = Arc::new(InMemoryObjectStore::default());
        for key in [
            "models/migration-fixture/model.json",
            "libraries/migration_fixture/releases/1.0.0/release.json",
            "system/library-rollouts/migration_fixture/1.0.0/deadbeef.json",
            "system/migrations/0001-model-project-bundles/items/migration-fixture.json",
            "system/migrations/0001-model-project-bundles/backup/migration-fixture/model.json",
            "system/migrations/0001-model-project-bundles.json",
        ] {
            store
                .put(key, Bytes::from_static(b"fixture"), PutCondition::Absent)
                .await
                .expect("fixture");
        }
        model_dependency_cutover::run(store.as_ref())
            .await
            .expect("cutover");
        assert_eq!(
            store.list("").await.expect("keys"),
            vec!["system/migrations/0001-model-project-bundles.json".to_owned()]
        );
        model_dependency_cutover::run(store.as_ref())
            .await
            .expect("idempotent cutover");

        let fresh = Arc::new(InMemoryObjectStore::default());
        run(fresh.clone()).await.expect("fresh store migrations");
        assert!(
            fresh
                .get("system/migrations/0001-model-project-bundles.json")
                .await
                .is_ok()
        );
        assert!(
            fresh
                .get("system/migrations/0002-model-dependency-cutover.json")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn model_dependency_cutover_restarts_after_interrupted_delete() {
        let store = Arc::new(InterruptDeleteStore::default());
        store
            .put(
                "libraries/migration_fixture/releases/1.0.0/release.json",
                Bytes::from_static(b"fixture"),
                PutCondition::Absent,
            )
            .await
            .expect("fixture");
        store.interrupt_next_delete.store(true, Ordering::SeqCst);

        assert_eq!(run(store.clone()).await, Err(RepositoryError::Unavailable));
        assert!(
            store
                .get("system/migrations/0001-model-project-bundles.json")
                .await
                .is_ok()
        );
        assert_eq!(
            store
                .get("system/migrations/0002-model-dependency-cutover.json")
                .await,
            Err(StorageError::NotFound)
        );

        run(store.clone()).await.expect("restart migration");
        assert!(
            store
                .list("libraries/")
                .await
                .expect("legacy keys")
                .is_empty()
        );
        assert!(
            store
                .get("system/migrations/0002-model-dependency-cutover.json")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn model_dependency_cutover_failure_matrix_is_restart_safe() {
        let delete_keys = [
            "models/part/model.json",
            "libraries/fixture/release.json",
            "system/library-rollouts/fixture/rollout.json",
            "system/migrations/0001-model-project-bundles/items/part.json",
            "system/migrations/0001-model-project-bundles/backup/part/model.json",
        ];
        for key in delete_keys {
            let store = pending_cutover_store().await;
            put_cutover_fixture(&store, key).await;
            store.fail_delete_once(key);
            assert_eq!(run(store.clone()).await, Err(RepositoryError::Unavailable));
            assert_cutover_ledger_absent(&store).await;
            restart_cutover(store).await;
        }

        for failed_list_call in [2, 3, 7] {
            let store = pending_cutover_store().await;
            if failed_list_call != 7 {
                put_cutover_fixture(&store, "models/part/model.json").await;
            }
            store.fail_list_once(failed_list_call);
            assert_eq!(run(store.clone()).await, Err(RepositoryError::Unavailable));
            assert_cutover_ledger_absent(&store).await;
            restart_cutover(store).await;
        }

        let paged = pending_cutover_store().await;
        put_cutover_fixture(&paged, "libraries/fixture/a").await;
        put_cutover_fixture(&paged, "libraries/fixture/b").await;
        *paged.paged_prefix.lock().expect("page fixture") = Some("libraries/".to_owned());
        restart_cutover(paged).await;

        let ledger_failure = pending_cutover_store().await;
        let completion_ledger_key = ledger_key(model_dependency_cutover::ID);
        ledger_failure.fail_put_once(&completion_ledger_key);
        assert_eq!(
            run(ledger_failure.clone()).await,
            Err(RepositoryError::Unavailable)
        );
        assert_cutover_ledger_absent(&ledger_failure).await;
        for prefix in model_dependency_cutover::PREFIXES {
            assert!(
                ledger_failure
                    .list(prefix)
                    .await
                    .expect("empty before completion retry")
                    .is_empty()
            );
        }
        restart_cutover(ledger_failure).await;
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
