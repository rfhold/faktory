//! Persisted model and named-view records.

use std::{
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use faktory_proto::v1::{
    Model, ModelGeometryFacts, NamedView, Projection, Quaternion, RenderState, Vector3,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, broadcast};

use crate::storage::{ObjectStore, PutCondition, StorageError};

pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_NAME_BYTES: usize = 200;
const MAX_ERROR_BYTES: usize = 300;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StoredRenderState {
    Pending,
    Rendering,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeometrySizeRecord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeometryFactsRecord {
    pub volume_cubic_millimeters: f64,
    pub size_millimeters: GeometrySizeRecord,
}

impl GeometryFactsRecord {
    const fn to_proto(self) -> ModelGeometryFacts {
        ModelGeometryFacts {
            volume_cubic_millimeters: self.volume_cubic_millimeters,
            size_millimeters: Some(Vector3 {
                x: self.size_millimeters.x,
                y: self.size_millimeters.y,
                z: self.size_millimeters.z,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimestampRecord {
    pub seconds: i64,
    pub nanos: i32,
}

impl TimestampRecord {
    const fn to_proto(self) -> prost_types::Timestamp {
        prost_types::Timestamp {
            seconds: self.seconds,
            nanos: self.nanos,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRecord {
    pub id: String,
    pub name: String,
    pub desired_source_revision: String,
    pub current_successful_source_revision: String,
    pub render_state: StoredRenderState,
    pub render_error: String,
    pub default_view_id: String,
    pub current_successful_facts: Option<GeometryFactsRecord>,
    pub updated_at: TimestampRecord,
}

impl ModelRecord {
    #[must_use]
    pub fn to_proto(&self) -> Model {
        Model {
            id: self.id.clone(),
            name: self.name.clone(),
            desired_source_revision: self.desired_source_revision.clone(),
            current_successful_source_revision: self.current_successful_source_revision.clone(),
            render_state: match self.render_state {
                StoredRenderState::Pending => RenderState::Pending,
                StoredRenderState::Rendering => RenderState::Rendering,
                StoredRenderState::Ready => RenderState::Ready,
                StoredRenderState::Failed => RenderState::Failed,
            }
            .into(),
            render_error: self.render_error.clone(),
            default_view_id: self.default_view_id.clone(),
            current_successful_facts: self
                .current_successful_facts
                .map(GeometryFactsRecord::to_proto),
            updated_at: Some(self.updated_at.to_proto()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedOutput {
    pub glb: Bytes,
    pub preview: Bytes,
    pub facts: GeometryFactsRecord,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRecord {
    pub id: String,
    pub name: String,
    pub target: [f64; 3],
    pub rotation: [f64; 4],
    pub projection: i32,
    pub distance: f64,
    pub field_of_view_degrees: f64,
    pub orthographic_scale: f64,
    pub etag: String,
}

impl ViewRecord {
    #[must_use]
    pub fn to_proto(&self) -> NamedView {
        NamedView {
            id: self.id.clone(),
            name: self.name.clone(),
            target: Some(Vector3 {
                x: self.target[0],
                y: self.target[1],
                z: self.target[2],
            }),
            rotation: Some(Quaternion {
                x: self.rotation[0],
                y: self.rotation[1],
                z: self.rotation[2],
                w: self.rotation[3],
            }),
            projection: self.projection,
            distance: self.distance,
            field_of_view_degrees: self.field_of_view_degrees,
            orthographic_scale: self.orthographic_scale,
            etag: self.etag.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, Eq, PartialEq)]
pub enum RepositoryError {
    #[error("invalid argument")]
    Invalid,
    #[error("record was not found")]
    NotFound,
    #[error("record changed concurrently")]
    Conflict,
    #[error("persistence is unavailable")]
    Unavailable,
    #[error("persisted record is invalid")]
    Corrupt,
}

impl From<StorageError> for RepositoryError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound => Self::NotFound,
            StorageError::Conflict => Self::Conflict,
            StorageError::Unavailable => Self::Unavailable,
            StorageError::Invalid => Self::Corrupt,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedModel {
    pub record: ModelRecord,
    storage_etag: String,
}

#[derive(Clone, Debug)]
pub struct LoadedView {
    pub record: ViewRecord,
    storage_etag: String,
}

#[derive(Clone, Debug)]
pub struct ModelChange(pub ModelRecord);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourcePatch {
    pub old: String,
    pub new: String,
}

#[derive(Clone, Debug)]
pub struct EditedModel {
    pub record: ModelRecord,
    pub source_changed: bool,
}

#[derive(Clone, Debug)]
pub struct Repository {
    store: Arc<dyn ObjectStore>,
    changes: broadcast::Sender<ModelChange>,
    mutations: Arc<Mutex<()>>,
    degraded: Arc<AtomicBool>,
}

impl Repository {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, watch_capacity: usize) -> Self {
        let (changes, _) = broadcast::channel(watch_capacity.max(1));
        Self {
            store,
            changes,
            mutations: Arc::new(Mutex::new(())),
            degraded: Arc::new(AtomicBool::new(false)),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ModelChange> {
        self.changes.subscribe()
    }

    pub async fn ready(&self) -> Result<(), RepositoryError> {
        if self.degraded.load(Ordering::SeqCst) {
            return Err(RepositoryError::Unavailable);
        }
        self.store.ready().await?;
        for key in self.model_keys().await? {
            let model = self.load_model_key(&key).await?;
            self.validate_graph(&model).await?;
        }
        Ok(())
    }

    pub(crate) fn mark_degraded(&self) {
        self.degraded.store(true, Ordering::SeqCst);
    }

    pub async fn list_models(&self) -> Result<Vec<ModelRecord>, RepositoryError> {
        let mut models = Vec::new();
        for key in self.model_keys().await? {
            models.push(self.load_model_key(&key).await?);
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(models)
    }

    pub async fn get_model(&self, model_id: &str) -> Result<LoadedModel, RepositoryError> {
        validate_model_id(model_id)?;
        let (record, storage_etag) = self.load_json::<ModelRecord>(&model_key(model_id)).await?;
        validate_model_record(&record, model_id)?;
        Ok(LoadedModel {
            record,
            storage_etag,
        })
    }

    pub async fn list_views(&self, model_id: &str) -> Result<Vec<ViewRecord>, RepositoryError> {
        self.get_model(model_id).await?;
        let prefix = format!("models/{model_id}/views/");
        let mut views = Vec::new();
        for key in self.store.list(&prefix).await? {
            let path = std::path::Path::new(&key);
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                let expected_id = path
                    .file_stem()
                    .and_then(std::ffi::OsStr::to_str)
                    .ok_or(RepositoryError::Corrupt)?;
                let record = self.load_json::<ViewRecord>(&key).await?.0;
                validate_view_record(&record, expected_id)?;
                views.push(record);
            }
        }
        views.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(views)
    }

    pub async fn put_view(
        &self,
        model_id: &str,
        view: NamedView,
        expected_etag: Option<&str>,
    ) -> Result<ViewRecord, RepositoryError> {
        let _guard = self.mutations.lock().await;
        self.get_model(model_id).await?;
        let creating = view.id.is_empty() && expected_etag.is_none();
        if !creating && (view.id.is_empty() || expected_etag.is_none()) {
            return Err(RepositoryError::Invalid);
        }
        let id = if creating {
            uuid::Uuid::new_v4().to_string()
        } else {
            view.id.clone()
        };
        validate_id(&id)?;
        let key = view_key(model_id, &id);
        let condition = if creating {
            PutCondition::Absent
        } else {
            let loaded: LoadedView = self.get_view(model_id, &id).await?;
            if Some(loaded.record.etag.as_str()) != expected_etag {
                return Err(RepositoryError::Conflict);
            }
            PutCondition::Matches(loaded.storage_etag)
        };
        let record = validate_view(view, id)?;
        self.put_json(&key, &record, condition).await?;
        Ok(record)
    }

    pub async fn get_view(
        &self,
        model_id: &str,
        view_id: &str,
    ) -> Result<LoadedView, RepositoryError> {
        validate_id(model_id)?;
        validate_id(view_id)?;
        let (record, storage_etag) = self.load_json(&view_key(model_id, view_id)).await?;
        validate_view_record(&record, view_id)?;
        Ok(LoadedView {
            record,
            storage_etag,
        })
    }

    pub async fn delete_view(
        &self,
        model_id: &str,
        view_id: &str,
        expected_etag: &str,
    ) -> Result<(), RepositoryError> {
        let _guard = self.mutations.lock().await;
        let view = self.get_view(model_id, view_id).await?;
        if view.record.etag != expected_etag {
            return Err(RepositoryError::Conflict);
        }
        let model = self.get_model(model_id).await?;
        if model.record.default_view_id == view_id {
            let mut record = model.record;
            record.default_view_id.clear();
            self.save_model(&record, PutCondition::Matches(model.storage_etag))
                .await?;
        }
        self.store
            .delete(&view_key(model_id, view_id), &view.storage_etag)
            .await?;
        Ok(())
    }

    pub async fn set_default_view(
        &self,
        model_id: &str,
        view_id: &str,
    ) -> Result<ModelRecord, RepositoryError> {
        let _guard = self.mutations.lock().await;
        self.get_view(model_id, view_id).await?;
        let model = self.get_model(model_id).await?;
        let mut record = model.record;
        record.default_view_id = view_id.to_owned();
        self.save_model(&record, PutCondition::Matches(model.storage_etag))
            .await?;
        Ok(record)
    }

    pub async fn create_model(
        &self,
        model_id: &str,
        name: &str,
        source: &[u8],
    ) -> Result<ModelRecord, RepositoryError> {
        validate_model_id(model_id)?;
        validate_name(name)?;
        validate_source(source)?;
        let _guard = self.mutations.lock().await;
        match self.get_model(model_id).await {
            Ok(_) => return Err(RepositoryError::Conflict),
            Err(RepositoryError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let revision = source_revision(source);
        let source_key = source_key(model_id, &revision);
        self.put_immutable(&source_key, Bytes::copy_from_slice(source))
            .await?;
        let record = ModelRecord {
            id: model_id.to_owned(),
            name: name.to_owned(),
            desired_source_revision: revision,
            current_successful_source_revision: String::new(),
            render_state: StoredRenderState::Pending,
            render_error: String::new(),
            default_view_id: String::new(),
            current_successful_facts: None,
            updated_at: mutation_timestamp(None)?,
        };
        self.save_model(&record, PutCondition::Absent).await?;
        Ok(record)
    }

    pub async fn edit_model(
        &self,
        model_id: &str,
        expected_revision: &str,
        name: Option<&str>,
        patches: Option<&[SourcePatch]>,
    ) -> Result<EditedModel, RepositoryError> {
        validate_model_id(model_id)?;
        validate_revision(expected_revision)?;
        if name.is_none() && patches.is_none() || patches.is_some_and(<[SourcePatch]>::is_empty) {
            return Err(RepositoryError::Invalid);
        }
        if let Some(name) = name {
            validate_name(name)?;
        }
        let _guard = self.mutations.lock().await;
        let loaded = self.get_model(model_id).await?;
        if loaded.record.desired_source_revision != expected_revision {
            return Err(RepositoryError::Conflict);
        }
        let mut record = loaded.record;
        let source_changed = if let Some(patches) = patches {
            let source = self.source(model_id, expected_revision).await?;
            let mut edited =
                String::from_utf8(source.to_vec()).map_err(|_| RepositoryError::Corrupt)?;
            let original = edited.clone();
            for patch in patches {
                if patch.old.is_empty()
                    || edited
                        .as_bytes()
                        .windows(patch.old.len())
                        .filter(|candidate| *candidate == patch.old.as_bytes())
                        .take(2)
                        .count()
                        != 1
                {
                    return Err(RepositoryError::Invalid);
                }
                edited = edited.replacen(&patch.old, &patch.new, 1);
            }
            if edited == original {
                return Err(RepositoryError::Invalid);
            }
            validate_source(edited.as_bytes())?;
            let revision = source_revision(edited.as_bytes());
            self.put_immutable(&source_key(model_id, &revision), Bytes::from(edited))
                .await?;
            record.desired_source_revision = revision;
            record.render_state = StoredRenderState::Pending;
            record.render_error.clear();
            true
        } else {
            false
        };
        if let Some(name) = name {
            if !source_changed && name == record.name {
                return Err(RepositoryError::Invalid);
            }
            record.name = name.to_owned();
        }
        record.updated_at = mutation_timestamp(Some(record.updated_at))?;
        self.save_model(&record, PutCondition::Matches(loaded.storage_etag))
            .await?;
        Ok(EditedModel {
            record,
            source_changed,
        })
    }

    pub async fn retry_render(&self, model_id: &str) -> Result<ModelRecord, RepositoryError> {
        let _guard = self.mutations.lock().await;
        let model = self.get_model(model_id).await?;
        if model.record.render_state != StoredRenderState::Failed {
            return Err(RepositoryError::Invalid);
        }
        let mut record = model.record;
        record.render_state = StoredRenderState::Pending;
        record.render_error.clear();
        self.save_model(&record, PutCondition::Matches(model.storage_etag))
            .await?;
        Ok(record)
    }

    pub async fn set_rendering(
        &self,
        model_id: &str,
        revision: &str,
    ) -> Result<bool, RepositoryError> {
        let _guard = self.mutations.lock().await;
        let loaded = self.get_model(model_id).await?;
        if loaded.record.desired_source_revision != revision
            || loaded.record.render_state != StoredRenderState::Pending
        {
            return Ok(false);
        }
        let mut record = loaded.record;
        record.render_state = StoredRenderState::Rendering;
        self.save_model(&record, PutCondition::Matches(loaded.storage_etag))
            .await?;
        Ok(true)
    }

    pub async fn complete_render(
        &self,
        model_id: &str,
        revision: &str,
        output: RenderedOutput,
    ) -> Result<(), RepositoryError> {
        validate_geometry_facts(output.facts)?;
        let _guard = self.mutations.lock().await;
        validate_revision(revision)?;
        let preflight = self.get_model(model_id).await?;
        if !can_complete_render(&preflight.record, revision, output.facts) {
            return Ok(());
        }
        self.put_immutable(&geometry_key(model_id, revision), output.glb.clone())
            .await?;
        self.put_immutable(&preview_key(model_id, revision), output.preview.clone())
            .await?;
        let loaded = self.get_model(model_id).await?;
        if !can_complete_render(&loaded.record, revision, output.facts) {
            return Ok(());
        }
        if loaded.record.render_state == StoredRenderState::Ready {
            return Ok(());
        }
        let mut record = loaded.record;
        record.render_state = StoredRenderState::Ready;
        record.current_successful_source_revision = revision.to_owned();
        record.current_successful_facts = Some(output.facts);
        record.render_error.clear();
        self.save_model(&record, PutCondition::Matches(loaded.storage_etag))
            .await
    }

    async fn put_immutable(&self, key: &str, candidate: Bytes) -> Result<(), RepositoryError> {
        match self
            .store
            .put(key, candidate.clone(), PutCondition::Absent)
            .await
        {
            Ok(_) => Ok(()),
            Err(StorageError::Conflict) => {
                let stored = self.store.get(key).await?;
                if stored.bytes == candidate {
                    Ok(())
                } else {
                    Err(RepositoryError::Conflict)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn fail_render(
        &self,
        model_id: &str,
        revision: &str,
        safe_error: &str,
    ) -> Result<(), RepositoryError> {
        let safe_error = truncate_utf8(safe_error, MAX_ERROR_BYTES);
        self.update_render(
            model_id,
            revision,
            StoredRenderState::Failed,
            None,
            Some(safe_error),
        )
        .await
    }

    pub async fn source(&self, model_id: &str, revision: &str) -> Result<Bytes, RepositoryError> {
        validate_model_id(model_id)?;
        validate_revision(revision)?;
        Ok(self.store.get(&source_key(model_id, revision)).await?.bytes)
    }

    pub async fn geometry(&self, model_id: &str, revision: &str) -> Result<Bytes, RepositoryError> {
        validate_revision(revision)?;
        let model = self.get_model(model_id).await?.record;
        if model.current_successful_source_revision != revision {
            return Err(RepositoryError::NotFound);
        }
        Ok(self
            .store
            .get(&geometry_key(model_id, revision))
            .await?
            .bytes)
    }

    pub async fn preview(&self, model_id: &str, revision: &str) -> Result<Bytes, RepositoryError> {
        validate_revision(revision)?;
        let model = self.get_model(model_id).await?.record;
        if model.current_successful_source_revision != revision {
            return Err(RepositoryError::NotFound);
        }
        Ok(self
            .store
            .get(&preview_key(model_id, revision))
            .await?
            .bytes)
    }

    pub async fn reconcile(&self) -> Result<Vec<(String, String)>, RepositoryError> {
        let mut jobs = Vec::new();
        for key in self.model_keys().await? {
            let mut model = match self.load_model_key(&key).await {
                Ok(model) => model,
                Err(
                    RepositoryError::Invalid | RepositoryError::Corrupt | RepositoryError::NotFound,
                ) => {
                    tracing::warn!("model excluded from startup reconciliation: invalid metadata");
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Err(error) = self.validate_graph(&model).await {
                match error {
                    RepositoryError::Invalid
                    | RepositoryError::Corrupt
                    | RepositoryError::NotFound => {
                        tracing::warn!(
                            "model excluded from startup reconciliation: invalid object graph"
                        );
                        continue;
                    }
                    error => return Err(error),
                }
            }
            if matches!(
                model.render_state,
                StoredRenderState::Pending | StoredRenderState::Rendering
            ) {
                let loaded = self.get_model(&model.id).await?;
                model.render_state = StoredRenderState::Pending;
                self.save_model(&model, PutCondition::Matches(loaded.storage_etag))
                    .await?;
                jobs.push((model.id, model.desired_source_revision));
            }
        }
        Ok(jobs)
    }

    async fn model_keys(&self) -> Result<Vec<String>, RepositoryError> {
        Ok(self
            .store
            .list("models/")
            .await?
            .into_iter()
            .filter(|key| key.ends_with("/model.json"))
            .collect())
    }

    async fn load_model_key(&self, key: &str) -> Result<ModelRecord, RepositoryError> {
        let model_id = key
            .strip_prefix("models/")
            .and_then(|key| key.strip_suffix("/model.json"))
            .filter(|model_id| !model_id.contains('/'))
            .ok_or(RepositoryError::Corrupt)?;
        let record = self.load_json::<ModelRecord>(key).await?.0;
        validate_model_record(&record, model_id)?;
        Ok(record)
    }

    async fn validate_graph(&self, model: &ModelRecord) -> Result<(), RepositoryError> {
        self.store
            .get(&source_key(&model.id, &model.desired_source_revision))
            .await?;
        if !model.current_successful_source_revision.is_empty() {
            self.store
                .get(&geometry_key(
                    &model.id,
                    &model.current_successful_source_revision,
                ))
                .await?;
            self.store
                .get(&preview_key(
                    &model.id,
                    &model.current_successful_source_revision,
                ))
                .await?;
        }
        if !model.default_view_id.is_empty() {
            self.get_view(&model.id, &model.default_view_id).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_render(
        &self,
        model_id: &str,
        revision: &str,
        state: StoredRenderState,
        successful_revision: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let _guard = self.mutations.lock().await;
        let loaded = self.get_model(model_id).await?;
        if loaded.record.desired_source_revision != revision {
            return Ok(());
        }
        let mut record = loaded.record;
        record.render_state = state;
        if let Some(revision) = successful_revision {
            record.current_successful_source_revision = revision.to_owned();
        }
        if let Some(error) = error {
            record.render_error = error.to_owned();
        }
        self.save_model(&record, PutCondition::Matches(loaded.storage_etag))
            .await
    }

    async fn save_model(
        &self,
        record: &ModelRecord,
        condition: PutCondition,
    ) -> Result<(), RepositoryError> {
        self.put_json(&model_key(&record.id), record, condition)
            .await?;
        drop(self.changes.send(ModelChange(record.clone())));
        Ok(())
    }

    async fn load_json<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<(T, String), RepositoryError> {
        let object = self.store.get(key).await?;
        let value = serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
        Ok((value, object.etag))
    }

    async fn put_json<T: Serialize + Sync>(
        &self,
        key: &str,
        value: &T,
        condition: PutCondition,
    ) -> Result<String, RepositoryError> {
        let bytes = serde_json::to_vec(value).map_err(|_| RepositoryError::Corrupt)?;
        self.store
            .put(key, Bytes::from(bytes), condition)
            .await
            .map_err(Into::into)
    }
}

fn validate_model_record(record: &ModelRecord, expected_id: &str) -> Result<(), RepositoryError> {
    let invalid_fields = validate_model_id(expected_id).is_err()
        || validate_model_id(&record.id).is_err()
        || validate_name(&record.name).is_err()
        || validate_revision(&record.desired_source_revision).is_err()
        || (!record.current_successful_source_revision.is_empty()
            && validate_revision(&record.current_successful_source_revision).is_err())
        || (!record.default_view_id.is_empty() && validate_id(&record.default_view_id).is_err());
    if invalid_fields
        || record.id != expected_id
        || record.render_error.len() > MAX_ERROR_BYTES
        || record.render_error.chars().any(char::is_control)
        || validate_timestamp(record.updated_at).is_err()
        || record
            .current_successful_facts
            .is_some_and(|facts| validate_geometry_facts(facts).is_err())
        || (record.current_successful_source_revision.is_empty()
            && record.current_successful_facts.is_some())
        || (!record.current_successful_source_revision.is_empty()
            && record.current_successful_facts.is_none())
        || (record.render_state != StoredRenderState::Failed && !record.render_error.is_empty())
        || (record.render_state == StoredRenderState::Failed && record.render_error.is_empty())
        || (record.render_state == StoredRenderState::Ready
            && record.current_successful_source_revision != record.desired_source_revision)
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok(())
}

pub(crate) fn validate_geometry_facts(facts: GeometryFactsRecord) -> Result<(), RepositoryError> {
    let values = [
        facts.volume_cubic_millimeters,
        facts.size_millimeters.x,
        facts.size_millimeters.y,
        facts.size_millimeters.z,
    ];
    if values
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
    {
        Ok(())
    } else {
        Err(RepositoryError::Corrupt)
    }
}

fn can_complete_render(record: &ModelRecord, revision: &str, facts: GeometryFactsRecord) -> bool {
    if record.desired_source_revision != revision {
        return false;
    }
    match record.render_state {
        StoredRenderState::Pending | StoredRenderState::Rendering => true,
        StoredRenderState::Ready => {
            record.current_successful_source_revision == revision
                && record.current_successful_facts == Some(facts)
        }
        StoredRenderState::Failed => false,
    }
}

const MIN_PROTO_TIMESTAMP_SECONDS: i64 = -62_135_596_800;
const MAX_PROTO_TIMESTAMP_SECONDS: i64 = 253_402_300_799;

fn validate_timestamp(timestamp: TimestampRecord) -> Result<(), RepositoryError> {
    if (MIN_PROTO_TIMESTAMP_SECONDS..=MAX_PROTO_TIMESTAMP_SECONDS).contains(&timestamp.seconds)
        && (0..1_000_000_000).contains(&timestamp.nanos)
    {
        Ok(())
    } else {
        Err(RepositoryError::Corrupt)
    }
}

fn mutation_timestamp(
    previous: Option<TimestampRecord>,
) -> Result<TimestampRecord, RepositoryError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RepositoryError::Unavailable)?;
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| RepositoryError::Unavailable)?;
    let nanos = i32::try_from(duration.subsec_nanos()).expect("nanoseconds fit in i32");
    let mut timestamp = TimestampRecord { seconds, nanos };
    if previous.is_some_and(|previous| timestamp <= previous) {
        let previous = previous.expect("checked as present");
        timestamp = if previous.nanos < 999_999_999 {
            TimestampRecord {
                seconds: previous.seconds,
                nanos: previous.nanos + 1,
            }
        } else {
            TimestampRecord {
                seconds: previous
                    .seconds
                    .checked_add(1)
                    .ok_or(RepositoryError::Unavailable)?,
                nanos: 0,
            }
        };
    }
    validate_timestamp(timestamp).map_err(|_| RepositoryError::Unavailable)?;
    Ok(timestamp)
}

fn validate_view_record(record: &ViewRecord, expected_id: &str) -> Result<(), RepositoryError> {
    let invalid_fields = validate_id(expected_id).is_err()
        || validate_id(&record.id).is_err()
        || validate_name(&record.name).is_err()
        || validate_id(&record.etag).is_err();
    let projection =
        Projection::try_from(record.projection).map_err(|_| RepositoryError::Corrupt)?;
    let finite = record
        .target
        .iter()
        .chain(&record.rotation)
        .all(|value| value.is_finite())
        && record.distance.is_finite()
        && record.field_of_view_degrees.is_finite()
        && record.orthographic_scale.is_finite();
    let rotation_magnitude = record
        .rotation
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if invalid_fields
        || record.id != expected_id
        || !finite
        || record.distance <= 0.0
        || !rotation_magnitude.is_finite()
        || rotation_magnitude <= f64::EPSILON
        || matches!(
            projection,
            Projection::Perspective
                if record.field_of_view_degrees <= 0.0
                    || record.field_of_view_degrees >= 180.0
        )
        || matches!(projection, Projection::Orthographic if record.orthographic_scale <= 0.0)
        || projection == Projection::Unspecified
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok(())
}

fn validate_view(view: NamedView, id: String) -> Result<ViewRecord, RepositoryError> {
    validate_name(&view.name)?;
    let target = view.target.ok_or(RepositoryError::Invalid)?;
    let rotation = view.rotation.ok_or(RepositoryError::Invalid)?;
    let values = [
        target.x, target.y, target.z, rotation.x, rotation.y, rotation.z, rotation.w,
    ];
    if values.iter().any(|value| !value.is_finite())
        || !view.distance.is_finite()
        || view.distance <= 0.0
    {
        return Err(RepositoryError::Invalid);
    }
    let magnitude = (rotation.x.mul_add(
        rotation.x,
        rotation.y.mul_add(
            rotation.y,
            rotation.z.mul_add(rotation.z, rotation.w * rotation.w),
        ),
    ))
    .sqrt();
    if !magnitude.is_finite() || magnitude <= f64::EPSILON {
        return Err(RepositoryError::Invalid);
    }
    let projection = Projection::try_from(view.projection).map_err(|_| RepositoryError::Invalid)?;
    match projection {
        Projection::Perspective
            if !view.field_of_view_degrees.is_finite()
                || view.field_of_view_degrees <= 0.0
                || view.field_of_view_degrees >= 180.0 =>
        {
            return Err(RepositoryError::Invalid);
        }
        Projection::Orthographic
            if !view.orthographic_scale.is_finite() || view.orthographic_scale <= 0.0 =>
        {
            return Err(RepositoryError::Invalid);
        }
        Projection::Unspecified => return Err(RepositoryError::Invalid),
        _ => {}
    }
    Ok(ViewRecord {
        id,
        name: view.name,
        target: [target.x, target.y, target.z],
        rotation: [
            rotation.x / magnitude,
            rotation.y / magnitude,
            rotation.z / magnitude,
            rotation.w / magnitude,
        ],
        projection: projection.into(),
        distance: view.distance,
        field_of_view_degrees: view.field_of_view_degrees,
        orthographic_scale: view.orthographic_scale,
        etag: uuid::Uuid::new_v4().to_string(),
    })
}

pub fn validate_id(id: &str) -> Result<(), RepositoryError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(RepositoryError::Invalid)
    } else {
        Ok(())
    }
}

pub fn validate_model_id(id: &str) -> Result<(), RepositoryError> {
    if id.is_empty()
        || id.len() > 64
        || id.starts_with('-')
        || id.ends_with('-')
        || id.as_bytes().windows(2).any(|pair| pair == b"--")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Err(RepositoryError::Invalid)
    } else {
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), RepositoryError> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        Err(RepositoryError::Invalid)
    } else {
        Ok(())
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

const fn validate_source(source: &[u8]) -> Result<(), RepositoryError> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES || std::str::from_utf8(source).is_err()
    {
        Err(RepositoryError::Invalid)
    } else {
        Ok(())
    }
}

fn source_revision(source: &[u8]) -> String {
    Sha256::digest(source)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        })
}

fn validate_revision(revision: &str) -> Result<(), RepositoryError> {
    if revision.len() == 64
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(RepositoryError::Invalid)
    }
}

#[must_use]
pub fn model_key(model_id: &str) -> String {
    format!("models/{model_id}/model.json")
}
#[must_use]
pub fn source_key(model_id: &str, revision: &str) -> String {
    format!("models/{model_id}/revisions/{revision}/source.py")
}
#[must_use]
pub fn geometry_key(model_id: &str, revision: &str) -> String {
    format!("models/{model_id}/revisions/{revision}/model.glb")
}
#[must_use]
pub fn preview_key(model_id: &str, revision: &str) -> String {
    format!("models/{model_id}/revisions/{revision}/preview.svg")
}
#[must_use]
pub fn view_key(model_id: &str, view_id: &str) -> String {
    format!("models/{model_id}/views/{view_id}.json")
}

#[allow(dead_code)]
const DEFAULT_RENDER_TIMEOUT: Duration = Duration::from_mins(2);

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::storage::{InMemoryObjectStore, StoredObject};

    #[derive(Debug, Default)]
    struct FailingStore {
        inner: InMemoryObjectStore,
        fail_model_put: AtomicBool,
        fail_delete: AtomicBool,
    }

    #[async_trait]
    impl ObjectStore for FailingStore {
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
            if key.ends_with("/model.json") && self.fail_model_put.swap(false, Ordering::SeqCst) {
                return Err(StorageError::Unavailable);
            }
            self.inner.put(key, bytes, condition).await
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            if self.fail_delete.swap(false, Ordering::SeqCst) {
                return Err(StorageError::Unavailable);
            }
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    fn repository() -> Repository {
        Repository::new(Arc::new(InMemoryObjectStore::default()), 8)
    }

    fn rendered(glb: &'static [u8]) -> RenderedOutput {
        RenderedOutput {
            glb: Bytes::from_static(glb),
            preview: Bytes::from_static(b"<svg></svg>"),
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 24.0,
                size_millimeters: GeometrySizeRecord {
                    x: 2.0,
                    y: 3.0,
                    z: 4.0,
                },
            },
        }
    }

    fn view(id: String) -> NamedView {
        NamedView {
            id,
            name: "Front".to_owned(),
            target: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            rotation: Some(Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            }),
            projection: Projection::Perspective.into(),
            distance: 10.0,
            field_of_view_degrees: 45.0,
            orthographic_scale: 1.0,
            etag: String::new(),
        }
    }

    #[test]
    fn model_ids_are_strict_lowercase_ascii_kebab_case() {
        for valid in [
            "part",
            "part-2",
            "123",
            "550e8400-e29b-41d4-a716-446655440000",
        ] {
            validate_model_id(valid).expect("valid model ID");
        }
        for invalid in [
            "",
            "Part",
            "part_name",
            "-part",
            "part-",
            "part--name",
            "caf\u{e9}",
        ] {
            assert_eq!(validate_model_id(invalid), Err(RepositoryError::Invalid));
        }
        assert_eq!(
            validate_model_id(&"a".repeat(65)),
            Err(RepositoryError::Invalid)
        );
    }

    #[tokio::test]
    async fn duplicate_create_does_not_mutate_model_or_write_source() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let created = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create model");
        let rejected_revision = source_revision(b"second");

        assert_eq!(
            repository.create_model("part", "Changed", b"second").await,
            Err(RepositoryError::Conflict)
        );
        assert_eq!(
            repository
                .get_model("part")
                .await
                .expect("existing model")
                .record,
            created
        );
        assert_eq!(
            store.get(&source_key("part", &rejected_revision)).await,
            Err(StorageError::NotFound)
        );
    }

    #[tokio::test]
    async fn create_requires_matching_orphaned_source_bytes() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let source = b"source";
        let revision = source_revision(source);
        store
            .put(
                &source_key("matching", &revision),
                Bytes::copy_from_slice(source),
                PutCondition::Absent,
            )
            .await
            .expect("store matching orphan");
        repository
            .create_model("matching", "Matching", source)
            .await
            .expect("accept matching orphan");

        store
            .put(
                &source_key("corrupt", &revision),
                Bytes::from_static(b"different"),
                PutCondition::Absent,
            )
            .await
            .expect("store corrupt orphan");
        assert_eq!(
            repository.create_model("corrupt", "Corrupt", source).await,
            Err(RepositoryError::Conflict)
        );
        assert!(matches!(
            repository.get_model("corrupt").await,
            Err(RepositoryError::NotFound)
        ));
    }

    #[tokio::test]
    async fn edit_requires_matching_orphaned_source_bytes() {
        for matching in [true, false] {
            let store = Arc::new(InMemoryObjectStore::default());
            let repository = Repository::new(store.clone(), 8);
            let model = repository
                .create_model("part", "Part", b"first")
                .await
                .expect("create model");
            let revision = source_revision(b"second");
            store
                .put(
                    &source_key(&model.id, &revision),
                    if matching {
                        Bytes::from_static(b"second")
                    } else {
                        Bytes::from_static(b"different")
                    },
                    PutCondition::Absent,
                )
                .await
                .expect("store orphaned revision");
            let result = repository
                .edit_model(
                    &model.id,
                    &model.desired_source_revision,
                    None,
                    Some(&[SourcePatch {
                        old: "first".to_owned(),
                        new: "second".to_owned(),
                    }]),
                )
                .await;
            if matching {
                assert_eq!(
                    result
                        .expect("accept matching orphan")
                        .record
                        .desired_source_revision,
                    revision
                );
            } else {
                assert!(matches!(result, Err(RepositoryError::Conflict)));
                let unchanged = repository.get_model(&model.id).await.expect("model").record;
                assert_eq!(
                    unchanged.desired_source_revision,
                    model.desired_source_revision
                );
                assert_eq!(unchanged.updated_at, model.updated_at);
            }
        }
    }

    #[tokio::test]
    async fn timestamps_change_only_for_accepted_model_mutations() {
        let repository = repository();
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        validate_timestamp(created.updated_at).expect("valid timestamp");
        assert_eq!(
            created.to_proto().updated_at,
            Some(created.updated_at.to_proto())
        );
        assert_eq!(
            repository
                .edit_model(
                    &created.id,
                    &created.desired_source_revision,
                    Some("Part"),
                    None,
                )
                .await
                .map(|edited| edited.record),
            Err(RepositoryError::Invalid)
        );
        repository
            .set_rendering(&created.id, &created.desired_source_revision)
            .await
            .expect("set rendering");
        repository
            .fail_render(&created.id, &created.desired_source_revision, "failure")
            .await
            .expect("fail render");
        let after_render = repository
            .get_model(&created.id)
            .await
            .expect("model")
            .record;
        assert_eq!(after_render.updated_at, created.updated_at);

        let edited = repository
            .edit_model(
                &created.id,
                &created.desired_source_revision,
                Some("Renamed"),
                None,
            )
            .await
            .expect("rename")
            .record;
        assert!(edited.updated_at > created.updated_at);

        let view = repository
            .put_view(&created.id, view(String::new()), None)
            .await
            .expect("create view");
        repository
            .set_default_view(&created.id, &view.id)
            .await
            .expect("set default view");
        assert_eq!(
            repository
                .get_model(&created.id)
                .await
                .expect("model")
                .record
                .updated_at,
            edited.updated_at
        );
    }

    #[tokio::test]
    async fn persisted_timestamps_require_valid_protobuf_ranges() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let loaded = repository.get_model(&model.id).await.expect("model");
        let mut invalid = loaded.record;
        invalid.updated_at.nanos = 1_000_000_000;
        repository
            .save_model(&invalid, PutCondition::Matches(loaded.storage_etag))
            .await
            .expect("store invalid fixture");
        assert!(matches!(
            repository.get_model(&model.id).await,
            Err(RepositoryError::Corrupt)
        ));
    }

    #[tokio::test]
    async fn render_errors_are_truncated_to_the_utf8_byte_limit() {
        let repository = repository();
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");

        repository
            .fail_render(
                &created.id,
                &created.desired_source_revision,
                &"é".repeat(MAX_ERROR_BYTES),
            )
            .await
            .expect("fail render");

        let failed = repository
            .get_model(&created.id)
            .await
            .expect("failed model")
            .record;
        assert_eq!(failed.render_error.len(), MAX_ERROR_BYTES);
        assert_eq!(failed.render_error, "é".repeat(MAX_ERROR_BYTES / 2));
    }

    #[tokio::test]
    async fn source_edit_applies_sequential_patches_and_preserves_last_good_revision() {
        let repository = repository();
        let created = repository
            .create_model("part", "Part", b"alpha beta")
            .await
            .expect("create model");
        assert!(created.current_successful_facts.is_none());
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered(b"glb"),
            )
            .await
            .expect("complete render");
        let successful = repository
            .get_model(&created.id)
            .await
            .expect("successful model")
            .record;
        assert_eq!(
            successful.current_successful_facts,
            Some(rendered(b"").facts)
        );
        assert_eq!(
            successful.to_proto().current_successful_facts,
            Some(rendered(b"").facts.to_proto())
        );

        let edited = repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                Some("Edited Part"),
                Some(&[
                    SourcePatch {
                        old: "alpha".to_owned(),
                        new: "gamma".to_owned(),
                    },
                    SourcePatch {
                        old: "gamma beta".to_owned(),
                        new: "final".to_owned(),
                    },
                ]),
            )
            .await
            .expect("edit model");

        assert!(edited.source_changed);
        assert!(edited.record.updated_at > successful.updated_at);
        assert_eq!(edited.record.name, "Edited Part");
        assert_eq!(edited.record.render_state, StoredRenderState::Pending);
        assert!(edited.record.render_error.is_empty());
        assert_eq!(
            edited.record.current_successful_source_revision,
            created.desired_source_revision
        );
        assert_eq!(
            repository
                .source("part", &edited.record.desired_source_revision)
                .await
                .expect("edited source"),
            Bytes::from_static(b"final")
        );
    }

    #[tokio::test]
    async fn source_edit_rejects_stale_ambiguous_absent_and_overall_noop_patches() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let created = repository
            .create_model("part", "Part", b"aaa same same")
            .await
            .expect("create model");
        let stale = "0".repeat(64);
        let cases = [
            (
                stale.as_str(),
                vec![SourcePatch {
                    old: "same".to_owned(),
                    new: "other".to_owned(),
                }],
                RepositoryError::Conflict,
            ),
            (
                created.desired_source_revision.as_str(),
                vec![SourcePatch {
                    old: "same".to_owned(),
                    new: "other".to_owned(),
                }],
                RepositoryError::Invalid,
            ),
            (
                created.desired_source_revision.as_str(),
                vec![SourcePatch {
                    old: "aa".to_owned(),
                    new: "other".to_owned(),
                }],
                RepositoryError::Invalid,
            ),
            (
                created.desired_source_revision.as_str(),
                vec![SourcePatch {
                    old: "missing".to_owned(),
                    new: "other".to_owned(),
                }],
                RepositoryError::Invalid,
            ),
            (
                created.desired_source_revision.as_str(),
                vec![
                    SourcePatch {
                        old: "aaa same same".to_owned(),
                        new: "changed".to_owned(),
                    },
                    SourcePatch {
                        old: "changed".to_owned(),
                        new: "aaa same same".to_owned(),
                    },
                ],
                RepositoryError::Invalid,
            ),
        ];
        for (revision, patches, expected) in cases {
            assert_eq!(
                repository
                    .edit_model("part", revision, None, Some(&patches))
                    .await
                    .map(|edited| edited.record),
                Err(expected)
            );
        }

        assert_eq!(
            repository
                .get_model("part")
                .await
                .expect("unchanged model")
                .record,
            created
        );
        assert_eq!(
            store
                .list("models/part/revisions/")
                .await
                .expect("revisions")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn name_only_edit_preserves_render_state_and_revisions() {
        let repository = repository();
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .fail_render(
                &created.id,
                &created.desired_source_revision,
                "safe failure",
            )
            .await
            .expect("fail render");
        let before = repository
            .get_model("part")
            .await
            .expect("failed model")
            .record;

        let edited = repository
            .edit_model(
                "part",
                &before.desired_source_revision,
                Some("Renamed"),
                None,
            )
            .await
            .expect("rename model");

        assert!(!edited.source_changed);
        assert_eq!(edited.record.name, "Renamed");
        assert_eq!(edited.record.render_state, before.render_state);
        assert_eq!(edited.record.render_error, before.render_error);
        assert_eq!(
            edited.record.desired_source_revision,
            before.desired_source_revision
        );
        assert_eq!(
            edited.record.current_successful_source_revision,
            before.current_successful_source_revision
        );
    }

    #[tokio::test]
    async fn failed_replacement_preserves_last_good_geometry() {
        let repository = repository();
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("accept first source");
        repository
            .complete_render(&first.id, &first.desired_source_revision, rendered(b"old"))
            .await
            .expect("complete first render");
        let second = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("accept replacement source")
            .record;
        repository
            .fail_render(&second.id, &second.desired_source_revision, "safe failure")
            .await
            .expect("fail replacement render");

        let geometry = repository
            .geometry(&first.id, &first.desired_source_revision)
            .await
            .expect("last good GLB");
        assert_eq!(geometry, Bytes::from_static(b"old"));
        let model = repository
            .get_model(&first.id)
            .await
            .expect("stored model")
            .record;
        assert_eq!(model.render_state, StoredRenderState::Failed);
        assert_eq!(model.render_error, "safe failure");
        assert_eq!(model.current_successful_facts, Some(rendered(b"").facts));
        assert_eq!(
            repository
                .preview(&first.id, &first.desired_source_revision)
                .await
                .expect("last good preview"),
            Bytes::from_static(b"<svg></svg>")
        );
    }

    #[tokio::test]
    async fn successful_replacement_advances_artifacts_and_facts_once() {
        let repository = repository();
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create");
        repository
            .complete_render(&first.id, &first.desired_source_revision, rendered(b"old"))
            .await
            .expect("first render");
        let second = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("edit")
            .record;
        let mut replacement = rendered(b"new");
        replacement.preview = Bytes::from_static(b"<svg><path/></svg>");
        replacement.facts.volume_cubic_millimeters = 48.0;
        repository
            .complete_render(
                &second.id,
                &second.desired_source_revision,
                replacement.clone(),
            )
            .await
            .expect("replacement render");

        let stored = repository
            .get_model(&second.id)
            .await
            .expect("model")
            .record;
        assert_eq!(
            stored.current_successful_source_revision,
            second.desired_source_revision
        );
        assert_eq!(stored.current_successful_facts, Some(replacement.facts));
        assert_eq!(
            repository
                .preview(&second.id, &second.desired_source_revision)
                .await
                .expect("preview"),
            replacement.preview
        );
    }

    #[tokio::test]
    async fn matching_immutable_conflicts_can_complete() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let output = rendered(b"glb");
        store
            .put(
                &geometry_key(&model.id, &model.desired_source_revision),
                output.glb.clone(),
                PutCondition::Absent,
            )
            .await
            .expect("store matching GLB");
        store
            .put(
                &preview_key(&model.id, &model.desired_source_revision),
                output.preview.clone(),
                PutCondition::Absent,
            )
            .await
            .expect("store matching preview");

        repository
            .complete_render(&model.id, &model.desired_source_revision, output.clone())
            .await
            .expect("complete matching retry");
        let completed = repository.get_model(&model.id).await.expect("model").record;
        assert_eq!(completed.render_state, StoredRenderState::Ready);
        assert_eq!(completed.current_successful_facts, Some(output.facts));
    }

    #[tokio::test]
    async fn mismatched_immutable_conflicts_cannot_advance_metadata() {
        for mismatch_preview in [false, true] {
            let store = Arc::new(InMemoryObjectStore::default());
            let repository = Repository::new(store.clone(), 8);
            let model = repository
                .create_model("part", "Part", b"source")
                .await
                .expect("create model");
            let output = rendered(b"candidate-glb");
            if mismatch_preview {
                store
                    .put(
                        &geometry_key(&model.id, &model.desired_source_revision),
                        output.glb.clone(),
                        PutCondition::Absent,
                    )
                    .await
                    .expect("store matching GLB");
                store
                    .put(
                        &preview_key(&model.id, &model.desired_source_revision),
                        Bytes::from_static(b"<svg><path/></svg>"),
                        PutCondition::Absent,
                    )
                    .await
                    .expect("store mismatched preview");
            } else {
                store
                    .put(
                        &geometry_key(&model.id, &model.desired_source_revision),
                        Bytes::from_static(b"different-glb"),
                        PutCondition::Absent,
                    )
                    .await
                    .expect("store mismatched GLB");
            }

            assert_eq!(
                repository
                    .complete_render(&model.id, &model.desired_source_revision, output)
                    .await,
                Err(RepositoryError::Conflict)
            );
            let unchanged = repository.get_model(&model.id).await.expect("model").record;
            assert_eq!(unchanged.render_state, StoredRenderState::Pending);
            assert!(unchanged.current_successful_source_revision.is_empty());
            assert!(unchanged.current_successful_facts.is_none());
        }
    }

    #[tokio::test]
    async fn stale_completion_does_not_replace_current_facts() {
        let repository = repository();
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create");
        let second = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("edit")
            .record;
        repository
            .complete_render(
                &first.id,
                &first.desired_source_revision,
                rendered(b"stale"),
            )
            .await
            .expect("ignore stale completion");

        let stored = repository.get_model(&first.id).await.expect("model").record;
        assert_eq!(
            stored.desired_source_revision,
            second.desired_source_revision
        );
        assert!(stored.current_successful_source_revision.is_empty());
        assert!(stored.current_successful_facts.is_none());
        assert_eq!(stored.render_state, StoredRenderState::Pending);
        assert_eq!(
            repository
                .store
                .get(&geometry_key(&first.id, &first.desired_source_revision))
                .await,
            Err(StorageError::NotFound)
        );
    }

    #[tokio::test]
    async fn orphaned_stale_artifacts_cannot_attach_unrelated_facts() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create model");
        let second = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("edit to second source")
            .record;
        store
            .put(
                &geometry_key(&first.id, &first.desired_source_revision),
                Bytes::from_static(b"stale-glb"),
                PutCondition::Absent,
            )
            .await
            .expect("store orphaned GLB");
        store
            .put(
                &preview_key(&first.id, &first.desired_source_revision),
                Bytes::from_static(b"<svg></svg>"),
                PutCondition::Absent,
            )
            .await
            .expect("store orphaned preview");
        let returned = repository
            .edit_model(
                &second.id,
                &second.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "second".to_owned(),
                    new: "first".to_owned(),
                }]),
            )
            .await
            .expect("return to first source")
            .record;
        let mut unrelated = rendered(b"new-glb");
        unrelated.facts.volume_cubic_millimeters = 999.0;

        assert_eq!(
            repository
                .complete_render(&returned.id, &returned.desired_source_revision, unrelated)
                .await,
            Err(RepositoryError::Conflict)
        );
        let unchanged = repository
            .get_model(&returned.id)
            .await
            .expect("model")
            .record;
        assert_eq!(unchanged.render_state, StoredRenderState::Pending);
        assert!(unchanged.current_successful_facts.is_none());
    }

    #[tokio::test]
    async fn first_render_failure_exposes_state_without_geometry() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        repository
            .fail_render(&model.id, &model.desired_source_revision, "safe failure")
            .await
            .expect("fail first render");

        assert_eq!(
            repository
                .geometry(&model.id, &model.desired_source_revision)
                .await,
            Err(RepositoryError::NotFound)
        );
        let model = repository
            .get_model(&model.id)
            .await
            .expect("stored model")
            .record;
        assert_eq!(model.render_state, StoredRenderState::Failed);
        assert_eq!(model.render_error, "safe failure");
        assert!(model.current_successful_source_revision.is_empty());
    }

    #[tokio::test]
    async fn view_updates_and_deletes_require_expected_logical_etag() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let created = repository
            .put_view(&model.id, view(String::new()), None)
            .await
            .expect("create view");
        assert_eq!(
            repository
                .put_view(&model.id, view(created.id.clone()), Some("stale"))
                .await,
            Err(RepositoryError::Conflict)
        );
        assert_eq!(
            repository
                .delete_view(&model.id, &created.id, "stale")
                .await,
            Err(RepositoryError::Conflict)
        );
        repository
            .delete_view(&model.id, &created.id, &created.etag)
            .await
            .expect("delete with expected etag");
    }

    #[tokio::test]
    async fn list_views_rejects_a_corrupt_non_default_view() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let valid = repository
            .put_view(&model.id, view(String::new()), None)
            .await
            .expect("create view");
        let mut corrupt = valid;
        corrupt.id = "different-id".to_owned();
        repository
            .put_json(
                &view_key(&model.id, "corrupt-id"),
                &corrupt,
                PutCondition::Absent,
            )
            .await
            .expect("store corrupt view");

        assert_eq!(
            repository.list_views(&model.id).await,
            Err(RepositoryError::Corrupt)
        );
        assert!(
            repository
                .get_model(&model.id)
                .await
                .expect("model")
                .record
                .default_view_id
                .is_empty()
        );
    }

    #[tokio::test]
    async fn delete_view_keeps_default_reference_when_clearing_it_fails() {
        let store = Arc::new(FailingStore::default());
        let repository = Repository::new(store.clone(), 8);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let created = repository
            .put_view(&model.id, view(String::new()), None)
            .await
            .expect("create view");
        repository
            .set_default_view(&model.id, &created.id)
            .await
            .expect("set default");
        store.fail_model_put.store(true, Ordering::SeqCst);

        assert_eq!(
            repository
                .delete_view(&model.id, &created.id, &created.etag)
                .await,
            Err(RepositoryError::Unavailable)
        );
        assert_eq!(
            repository
                .get_model(&model.id)
                .await
                .expect("model")
                .record
                .default_view_id,
            created.id
        );
        repository
            .get_view(&model.id, &created.id)
            .await
            .expect("referenced view remains");
    }

    #[tokio::test]
    async fn delete_view_failure_leaves_an_unreferenced_retryable_view() {
        let store = Arc::new(FailingStore::default());
        let repository = Repository::new(store.clone(), 8);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let created = repository
            .put_view(&model.id, view(String::new()), None)
            .await
            .expect("create view");
        repository
            .set_default_view(&model.id, &created.id)
            .await
            .expect("set default");
        store.fail_delete.store(true, Ordering::SeqCst);

        assert_eq!(
            repository
                .delete_view(&model.id, &created.id, &created.etag)
                .await,
            Err(RepositoryError::Unavailable)
        );
        assert!(
            repository
                .get_model(&model.id)
                .await
                .expect("model")
                .record
                .default_view_id
                .is_empty()
        );
        repository
            .get_view(&model.id, &created.id)
            .await
            .expect("unreferenced view remains");
        repository
            .delete_view(&model.id, &created.id, &created.etag)
            .await
            .expect("retry deletion");
    }

    #[tokio::test]
    async fn reconciliation_requeues_interrupted_desired_revision() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        repository
            .set_rendering(&model.id, &model.desired_source_revision)
            .await
            .expect("mark rendering");
        assert_eq!(
            repository.reconcile().await.expect("reconcile"),
            vec![(model.id.clone(), model.desired_source_revision.clone())]
        );
        assert_eq!(
            repository
                .get_model(&model.id)
                .await
                .expect("model")
                .record
                .render_state,
            StoredRenderState::Pending
        );
    }

    #[tokio::test]
    async fn reconciliation_isolates_invalid_models_and_requeues_desired_revision() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 8);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        repository
            .complete_render(&model.id, &model.desired_source_revision, rendered(b"glb"))
            .await
            .expect("complete render");
        let pending = repository
            .edit_model(
                &model.id,
                &model.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "source".to_owned(),
                    new: "updated source".to_owned(),
                }]),
            )
            .await
            .expect("edit source")
            .record;
        store
            .put(
                "models/corrupt/model.json",
                Bytes::from_static(b"not json"),
                PutCondition::Absent,
            )
            .await
            .expect("corrupt fixture");
        let missing_revision = "0".repeat(64);
        let missing = ModelRecord {
            id: "missing".to_owned(),
            name: "Missing".to_owned(),
            desired_source_revision: missing_revision,
            current_successful_source_revision: String::new(),
            render_state: StoredRenderState::Pending,
            render_error: String::new(),
            default_view_id: String::new(),
            current_successful_facts: None,
            updated_at: model.updated_at,
        };
        store
            .put(
                &model_key(&missing.id),
                Bytes::from(serde_json::to_vec(&missing).expect("serialize fixture")),
                PutCondition::Absent,
            )
            .await
            .expect("missing source fixture");

        assert_eq!(
            repository
                .reconcile()
                .await
                .expect("reconcile valid models"),
            vec![(pending.id, pending.desired_source_revision)]
        );
        assert_eq!(repository.ready().await, Err(RepositoryError::Corrupt));
    }

    #[tokio::test]
    async fn readiness_rejects_a_current_revision_without_geometry() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 8);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let loaded = repository.get_model(&model.id).await.expect("model");
        let mut invalid = loaded.record;
        invalid.current_successful_source_revision = invalid.desired_source_revision.clone();
        invalid.current_successful_facts = Some(rendered(b"").facts);
        invalid.render_state = StoredRenderState::Ready;
        repository
            .save_model(&invalid, PutCondition::Matches(loaded.storage_etag))
            .await
            .expect("store fixture");

        assert_eq!(repository.ready().await, Err(RepositoryError::NotFound));
        repository
            .store
            .put(
                &geometry_key(&model.id, &model.desired_source_revision),
                Bytes::from_static(b"glb"),
                PutCondition::Absent,
            )
            .await
            .expect("store geometry fixture");
        assert_eq!(repository.ready().await, Err(RepositoryError::NotFound));
        repository
            .store
            .put(
                &preview_key(&model.id, &model.desired_source_revision),
                Bytes::from_static(b"<svg></svg>"),
                PutCondition::Absent,
            )
            .await
            .expect("store preview fixture");
        repository.ready().await.expect("complete graph");
        assert!(
            repository
                .reconcile()
                .await
                .expect("isolate model")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn readiness_rejects_successful_metadata_without_facts() {
        let repository = repository();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .complete_render(&model.id, &model.desired_source_revision, rendered(b"glb"))
            .await
            .expect("complete render");
        let loaded = repository.get_model(&model.id).await.expect("model");
        let mut invalid = loaded.record;
        invalid.current_successful_facts = None;
        repository
            .save_model(&invalid, PutCondition::Matches(loaded.storage_etag))
            .await
            .expect("store invalid fixture");

        assert_eq!(repository.ready().await, Err(RepositoryError::Corrupt));
    }
}
