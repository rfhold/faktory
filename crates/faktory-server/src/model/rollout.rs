//! Durable idempotent same-major model-release consumer rollouts.

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use super::{
    Repository, RepositoryError,
    project::{ModelLock, ProjectEdit},
    release::ModelRelease,
    validate_model_id,
};
use crate::storage::{PutCondition, StorageError};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutModelState {
    Updated,
    AlreadyCurrent,
    NotEligible,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReleaseRolloutRecord {
    pub model_id: String,
    pub version: Version,
    pub release_sha256: String,
    pub complete: bool,
    pub models: BTreeMap<String, RolloutModelState>,
}

#[must_use]
pub fn rollout_key(model_id: &str, version: &Version, release_sha256: &str) -> String {
    format!("system/model-release-rollouts/{model_id}/{version}/{release_sha256}.json")
}

impl Repository {
    pub async fn reconcile_model_release_rollout_intents(
        &self,
    ) -> Result<Vec<ModelReleaseRolloutRecord>, RepositoryError> {
        let mut model_ids = self
            .store
            .list("models/")
            .await?
            .into_iter()
            .filter_map(|key| {
                let remainder = key.strip_prefix("models/")?;
                let (model_id, suffix) = remainder.split_once('/')?;
                suffix.starts_with("releases/").then(|| model_id.to_owned())
            })
            .collect::<Vec<_>>();
        model_ids.sort();
        model_ids.dedup();
        let mut intents = Vec::new();
        for model_id in model_ids {
            let releases = self.list_model_releases(&model_id).await?;
            for release in &releases {
                if releases.iter().any(|candidate| {
                    candidate.version < release.version
                        && candidate.version.major == release.version.major
                }) {
                    intents.push(self.create_rollout_intent(release).await?);
                }
            }
        }
        Ok(intents)
    }

    #[allow(clippy::suspicious_operation_groupings)]
    pub(super) async fn create_rollout_intent(
        &self,
        release: &ModelRelease,
    ) -> Result<ModelReleaseRolloutRecord, RepositoryError> {
        let record = ModelReleaseRolloutRecord {
            model_id: release.model_id.clone(),
            version: release.version.clone(),
            release_sha256: release.digest.clone(),
            complete: false,
            models: BTreeMap::new(),
        };
        let key = rollout_key(&release.model_id, &release.version, &release.digest);
        let bytes = serde_json::to_vec(&record).map_err(|_| RepositoryError::Corrupt)?;
        match self
            .store
            .put(&key, bytes.clone().into(), PutCondition::Absent)
            .await
        {
            Ok(_) => Ok(record),
            Err(StorageError::Conflict) => {
                let existing = self.load_json::<ModelReleaseRolloutRecord>(&key).await?.0;
                if existing.model_id != release.model_id
                    || existing.version != release.version
                    || existing.release_sha256 != release.digest
                {
                    Err(RepositoryError::Conflict)
                } else {
                    Ok(existing)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    #[allow(clippy::suspicious_operation_groupings, clippy::too_many_lines)]
    pub async fn rollout_model_release(
        &self,
        release: &ModelRelease,
    ) -> Result<ModelReleaseRolloutRecord, RepositoryError> {
        let stored = self
            .get_model_release(&release.model_id, &release.version)
            .await?;
        if stored.digest != release.digest {
            return Err(RepositoryError::Conflict);
        }
        let key = rollout_key(&release.model_id, &release.version, &release.digest);
        let mut record = match self.load_json::<ModelReleaseRolloutRecord>(&key).await {
            Ok((record, _)) => record,
            Err(RepositoryError::NotFound) => self.create_rollout_intent(release).await?,
            Err(error) => return Err(error),
        };
        if (record.model_id != release.model_id)
            || (record.version != release.version)
            || (record.release_sha256 != release.digest)
        {
            return Err(RepositoryError::Corrupt);
        }
        record.complete = false;
        self.persist_rollout(&key, &record).await?;
        for listed in self.list_models().await? {
            if record.models.contains_key(&listed.id) {
                continue;
            }
            let outcome = self
                .apply_model_release_to_consumer(&listed.id, release)
                .await?;
            record.models.insert(listed.id, outcome);
            self.persist_rollout(&key, &record).await?;
        }
        record.complete = true;
        self.persist_rollout(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn apply_model_release_to_consumer(
        &self,
        consumer_id: &str,
        release: &ModelRelease,
    ) -> Result<RolloutModelState, RepositoryError> {
        let _guard = self.mutations.lock().await;
        if consumer_id == release.model_id {
            return Ok(RolloutModelState::NotEligible);
        }
        let model = self.get_model(consumer_id).await?.record;
        let project = self
            .get_project(consumer_id, &model.desired_source_revision)
            .await?;
        let Some(requirement) = project
            .requirements
            .iter()
            .find(|item| item.model_id == release.model_id)
        else {
            return Ok(RolloutModelState::NotEligible);
        };
        if !requirement.matches(&release.version) {
            return Ok(RolloutModelState::NotEligible);
        }
        let current = project
            .locks
            .iter()
            .find(|item| item.model_id == release.model_id)
            .ok_or(RepositoryError::Corrupt)?;
        if current.version >= release.version {
            return Ok(RolloutModelState::AlreadyCurrent);
        }
        let mut locks = project.locks.clone();
        *locks
            .iter_mut()
            .find(|item| item.model_id == release.model_id)
            .ok_or(RepositoryError::Corrupt)? = ModelLock {
            model_id: release.model_id.clone(),
            version: release.version.clone(),
            project_revision: release.project_revision.clone(),
            release_sha256: release.digest.clone(),
        };
        let guidance = self.dependency_guidance(&locks).await?;
        self.edit_project_locked(
            consumer_id,
            &model.desired_source_revision,
            None,
            Some(&ProjectEdit::lock_update(locks, guidance)),
        )
        .await?;
        Ok(RolloutModelState::Updated)
    }

    pub async fn resume_incomplete_model_release_rollouts(
        &self,
    ) -> Result<Vec<ModelReleaseRolloutRecord>, RepositoryError> {
        let mut keys = self.store.list("system/model-release-rollouts/").await?;
        keys.sort();
        let mut resumed = Vec::new();
        for key in keys {
            let (model_id, version, digest) = parse_rollout_key(&key)?;
            let record = self.load_json::<ModelReleaseRolloutRecord>(&key).await?.0;
            if record.model_id != model_id
                || record.version != version
                || record.release_sha256 != digest
            {
                return Err(RepositoryError::Corrupt);
            }
            if record.complete {
                continue;
            }
            let release = self.get_model_release(&model_id, &version).await?;
            if release.digest != digest {
                return Err(RepositoryError::Corrupt);
            }
            resumed.push(self.rollout_model_release(&release).await?);
        }
        Ok(resumed)
    }

    async fn persist_rollout(
        &self,
        key: &str,
        record: &ModelReleaseRolloutRecord,
    ) -> Result<(), RepositoryError> {
        self.store
            .put(
                key,
                serde_json::to_vec(record)
                    .map_err(|_| RepositoryError::Corrupt)?
                    .into(),
                PutCondition::Any,
            )
            .await
            .map(|_| ())
            .map_err(Into::into)
    }
}

fn parse_rollout_key(key: &str) -> Result<(String, Version, String), RepositoryError> {
    let mut parts = key
        .strip_prefix("system/model-release-rollouts/")
        .ok_or(RepositoryError::Corrupt)?
        .split('/');
    let model_id = parts.next().ok_or(RepositoryError::Corrupt)?;
    let version = parts
        .next()
        .and_then(|value| Version::parse(value).ok())
        .ok_or(RepositoryError::Corrupt)?;
    let digest = parts
        .next()
        .and_then(|value| value.strip_suffix(".json"))
        .ok_or(RepositoryError::Corrupt)?;
    if parts.next().is_some()
        || validate_model_id(model_id).is_err()
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok((model_id.to_owned(), version, digest.to_owned()))
}
