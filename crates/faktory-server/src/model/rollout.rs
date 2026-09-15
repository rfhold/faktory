//! Durable idempotent shared-library consumer rollouts.

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use super::{
    Repository, RepositoryError,
    library::LibraryRelease,
    project::{LibraryLock, ProjectEdit, validate_library_name},
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
pub struct LibraryRolloutRecord {
    pub library_name: String,
    pub version: Version,
    pub release_sha256: String,
    pub complete: bool,
    pub models: BTreeMap<String, RolloutModelState>,
}

#[must_use]
pub fn rollout_key(name: &str, version: &Version, release_sha256: &str) -> String {
    format!("system/library-rollouts/{name}/{version}/{release_sha256}.json")
}

impl Repository {
    pub(super) async fn create_rollout_intent(
        &self,
        release: &LibraryRelease,
    ) -> Result<LibraryRolloutRecord, RepositoryError> {
        let record = LibraryRolloutRecord {
            library_name: release.name.clone(),
            version: release.version.clone(),
            release_sha256: release.digest.clone(),
            complete: false,
            models: BTreeMap::new(),
        };
        let key = rollout_key(&release.name, &release.version, &release.digest);
        let bytes = serde_json::to_vec(&record).map_err(|_| RepositoryError::Corrupt)?;
        match self
            .store
            .put(&key, bytes.clone().into(), PutCondition::Absent)
            .await
        {
            Ok(_) => Ok(record),
            Err(StorageError::Conflict) => {
                let existing = self.store.get(&key).await?;
                if existing.bytes == bytes {
                    Ok(record)
                } else {
                    Err(RepositoryError::Conflict)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn rollout_library_release(
        &self,
        release: &LibraryRelease,
    ) -> Result<LibraryRolloutRecord, RepositoryError> {
        let stored = self.get_library(&release.name, &release.version).await?;
        if stored.digest != release.digest {
            return Err(RepositoryError::Conflict);
        }
        let key = rollout_key(&release.name, &release.version, &release.digest);
        let mut record = match self.load_json::<LibraryRolloutRecord>(&key).await {
            Ok((record, _)) => record,
            Err(RepositoryError::NotFound) => self.create_rollout_intent(release).await?,
            Err(error) => return Err(error),
        };
        let identity_matches = record.library_name.eq(&release.name)
            && record.version.eq(&release.version)
            && record.release_sha256.eq(&release.digest);
        if !identity_matches {
            return Err(RepositoryError::Corrupt);
        }
        record.complete = false;
        self.persist_rollout(&key, &record).await?;

        for listed in self.list_models().await? {
            let mut attempts = 0;
            let outcome = loop {
                attempts += 1;
                let model = self.get_model(&listed.id).await?.record;
                let project = self
                    .get_project(&model.id, &model.desired_source_revision)
                    .await?;
                let Some(requirement) = project
                    .requirements
                    .iter()
                    .find(|requirement| requirement.name == release.name)
                else {
                    break RolloutModelState::NotEligible;
                };
                if !requirement.matches(&release.version) {
                    break RolloutModelState::NotEligible;
                }
                let current = project
                    .locks
                    .iter()
                    .find(|lock| lock.name == release.name)
                    .ok_or(RepositoryError::Corrupt)?;
                if current.version >= release.version {
                    break RolloutModelState::AlreadyCurrent;
                }
                let mut locks = project.locks.clone();
                *locks
                    .iter_mut()
                    .find(|lock| lock.name == release.name)
                    .ok_or(RepositoryError::Corrupt)? = LibraryLock {
                    name: release.name.clone(),
                    version: release.version.clone(),
                    release_sha256: release.digest.clone(),
                };
                let guidance = self.dependency_guidance(&locks).await?;
                let edit = ProjectEdit::lock_update(locks, guidance);
                match self
                    .edit_project(&model.id, &model.desired_source_revision, None, Some(&edit))
                    .await
                {
                    Ok(_) => break RolloutModelState::Updated,
                    Err(RepositoryError::Conflict) if attempts < 4 => {}
                    Err(RepositoryError::Conflict) => return Err(RepositoryError::Conflict),
                    Err(error) => return Err(error),
                }
            };
            record.models.insert(listed.id, outcome);
            self.persist_rollout(&key, &record).await?;
        }
        record.complete = true;
        self.persist_rollout(&key, &record).await?;
        Ok(record)
    }

    pub async fn resume_incomplete_library_rollouts(
        &self,
    ) -> Result<Vec<LibraryRolloutRecord>, RepositoryError> {
        let mut keys = self.store.list("system/library-rollouts/").await?;
        keys.sort();
        let mut resumed = Vec::new();
        for key in keys {
            let (name, version, release_sha256) = parse_rollout_key(&key)?;
            let (record, _) = self.load_json::<LibraryRolloutRecord>(&key).await?;
            if record.library_name != name
                || record.version != version
                || record.release_sha256 != release_sha256
            {
                return Err(RepositoryError::Corrupt);
            }
            if record.complete {
                continue;
            }
            let release = self.get_library(&name, &version).await?;
            if release.digest != release_sha256 {
                return Err(RepositoryError::Corrupt);
            }
            resumed.push(self.rollout_library_release(&release).await?);
        }
        Ok(resumed)
    }

    pub async fn get_rollout(
        &self,
        name: &str,
        version: &Version,
        release_sha256: &str,
    ) -> Result<LibraryRolloutRecord, RepositoryError> {
        Ok(self
            .load_json::<LibraryRolloutRecord>(&rollout_key(name, version, release_sha256))
            .await?
            .0)
    }

    async fn persist_rollout(
        &self,
        key: &str,
        record: &LibraryRolloutRecord,
    ) -> Result<(), RepositoryError> {
        let bytes = serde_json::to_vec(record).map_err(|_| RepositoryError::Corrupt)?;
        self.store
            .put(key, bytes.into(), PutCondition::Any)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }
}

fn parse_rollout_key(key: &str) -> Result<(String, Version, String), RepositoryError> {
    let mut parts = key
        .strip_prefix("system/library-rollouts/")
        .ok_or(RepositoryError::Corrupt)?
        .split('/');
    let name = parts.next().ok_or(RepositoryError::Corrupt)?;
    let version = parts
        .next()
        .and_then(|value| Version::parse(value).ok())
        .ok_or(RepositoryError::Corrupt)?;
    let release_sha256 = parts
        .next()
        .and_then(|value| value.strip_suffix(".json"))
        .ok_or(RepositoryError::Corrupt)?;
    if parts.next().is_some()
        || validate_library_name(name).is_err()
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || release_sha256.len() != 64
        || !release_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok((name.to_owned(), version, release_sha256.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            GeometryFactsRecord, GeometrySizeRecord, RenderedOutput, TechnicalProjectionImages,
            library::LibraryRelease,
            project::{
                DependencyGuidance, DirectRequirement, LibraryLock, ProjectBundle, ProjectFile,
            },
        },
        storage::InMemoryObjectStore,
    };
    use std::sync::Arc;

    fn file(path: &str, content: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            content: content.to_owned(),
        }
    }
    fn release(version: Version) -> LibraryRelease {
        LibraryRelease::new(
            "gears".to_owned(),
            version,
            vec![file("faktory_shared/gears/__init__.py", "")],
            "Use it".to_owned(),
            vec![file("docs/guide.md", "API")],
        )
        .expect("release")
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn rollout_updates_desired_lock_and_preserves_last_good() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 4);
        assert_eq!(
            rollout_key("gears", &Version::new(1, 1, 0), "abc"),
            "system/library-rollouts/gears/1.1.0/abc.json"
        );
        let old = repository
            .publish_library(release(Version::new(1, 0, 0)))
            .await
            .expect("old");
        let requirement = DirectRequirement {
            name: "gears".to_owned(),
            range: ">=1.0.0,<2.0.0".to_owned(),
        };
        let guidance = vec![DependencyGuidance {
            name: "gears".to_owned(),
            version: old.version.clone(),
            release_sha256: old.digest.clone(),
            guidance: old.guidance.clone(),
            documentation: old.docs.iter().map(|doc| doc.path.clone()).collect(),
        }];
        let project = ProjectBundle::new(
            vec![file("main.py", "part = 1")],
            "main.py".to_owned(),
            vec![requirement],
            vec![LibraryLock {
                name: "gears".to_owned(),
                version: old.version.clone(),
                release_sha256: old.digest.clone(),
            }],
            &guidance,
            "",
        )
        .expect("project");
        let model = repository
            .create_project("consumer", "Consumer", project)
            .await
            .expect("model");
        repository
            .complete_render(
                &model.id,
                &model.desired_source_revision,
                RenderedOutput {
                    glb: bytes::Bytes::from_static(b"glb"),
                    preview: bytes::Bytes::from_static(b"svg"),
                    facts: GeometryFactsRecord {
                        volume_cubic_millimeters: 1.0,
                        size_millimeters: GeometrySizeRecord {
                            x: 1.0,
                            y: 1.0,
                            z: 1.0,
                        },
                    },
                    projections: TechnicalProjectionImages::all(bytes::Bytes::from_static(b"png")),
                    shaded: TechnicalProjectionImages::all(bytes::Bytes::from_static(b"png")),
                },
            )
            .await
            .expect("render");
        let next = repository
            .publish_library(release(Version::new(1, 1, 0)))
            .await
            .expect("next");
        assert!(
            !repository
                .get_rollout("gears", &next.version, &next.digest)
                .await
                .expect("intent")
                .complete
        );
        let first = repository
            .rollout_library_release(&next)
            .await
            .expect("rollout");
        assert_eq!(first.models["consumer"], RolloutModelState::Updated);
        let changed = repository
            .get_model("consumer")
            .await
            .expect("consumer")
            .record;
        assert_ne!(
            changed.desired_source_revision,
            model.desired_source_revision
        );
        assert_eq!(
            changed.current_successful_source_revision,
            model.desired_source_revision
        );
        let rerun = repository
            .rollout_library_release(&next)
            .await
            .expect("rerun");
        assert_eq!(rerun.models["consumer"], RolloutModelState::AlreadyCurrent);
        assert_eq!(
            repository
                .get_model("consumer")
                .await
                .expect("consumer")
                .record
                .desired_source_revision,
            changed.desired_source_revision
        );

        let one_two = repository
            .publish_library(release(Version::new(1, 2, 0)))
            .await
            .expect("1.2");
        let one_three = repository
            .publish_library(release(Version::new(1, 3, 0)))
            .await
            .expect("1.3");
        let resumed = repository
            .resume_incomplete_library_rollouts()
            .await
            .expect("resume");
        assert_eq!(
            resumed
                .iter()
                .map(|record| record.version.clone())
                .collect::<Vec<_>>(),
            vec![one_two.version, one_three.version.clone()]
        );
        assert!(resumed.iter().all(|record| record.complete));
        let latest = repository
            .get_model("consumer")
            .await
            .expect("consumer")
            .record;
        assert_eq!(
            repository
                .get_project("consumer", &latest.desired_source_revision)
                .await
                .expect("project")
                .locks[0]
                .version,
            one_three.version
        );
        assert_eq!(
            latest.current_successful_source_revision,
            model.desired_source_revision
        );
        assert!(
            repository
                .resume_incomplete_library_rollouts()
                .await
                .expect("idempotent resume")
                .is_empty()
        );
        assert_eq!(
            repository
                .get_model("consumer")
                .await
                .expect("consumer")
                .record
                .desired_source_revision,
            latest.desired_source_revision
        );
    }
}
