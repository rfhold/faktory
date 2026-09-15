//! Immutable direct-only shared-library releases.

use std::collections::BTreeSet;

use bytes::Bytes;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    Repository, RepositoryError,
    project::{
        ProjectFile, hex_digest, markdown_heading_level, normalize_text, normalize_user_path,
        parse_requirement, validate_library_name,
    },
};
use crate::storage::PutCondition;

const FORMAT: &str = "faktory-library-v1";
const MAX_LIBRARY_FILES: usize = 256;
const MAX_LIBRARY_DOCS: usize = 64;
const MAX_LIBRARY_FILE_BYTES: usize = 1_048_576;
const MAX_LIBRARY_CONTENT_BYTES: usize = 1_048_576;
const MAX_LIBRARY_BUNDLE_BYTES: usize = 8_388_608;
const MAX_GUIDANCE_BYTES: usize = 16_384;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryRelease {
    pub format: String,
    pub name: String,
    pub version: Version,
    pub guidance: String,
    pub docs: Vec<ProjectFile>,
    pub files: Vec<ProjectFile>,
    #[serde(skip)]
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryCatalogEntry {
    pub name: String,
    pub package: String,
    pub versions: Vec<Version>,
}

impl LibraryRelease {
    pub fn new(
        name: String,
        version: Version,
        files: Vec<ProjectFile>,
        guidance: String,
        docs: Vec<ProjectFile>,
    ) -> Result<Self, RepositoryError> {
        validate_library_name(&name)?;
        if !version.pre.is_empty()
            || !version.build.is_empty()
            || files.is_empty()
            || files.len() > MAX_LIBRARY_FILES
            || docs.is_empty()
            || docs.len() > MAX_LIBRARY_DOCS
        {
            return Err(RepositoryError::Invalid);
        }
        let guidance = normalize_text(&guidance).trim_end_matches('\n').to_owned();
        if guidance.is_empty()
            || guidance.len() > MAX_GUIDANCE_BYTES
            || guidance
                .lines()
                .any(|line| markdown_heading_level(line).is_some_and(|level| level <= 2))
        {
            return Err(RepositoryError::Invalid);
        }
        let package_path = format!("faktory_shared/{name}");
        let mut files = normalize_release_files(files, &format!("{package_path}/"), true)?;
        let mut docs = normalize_release_files(docs, "docs/", false)?;
        if !files
            .iter()
            .any(|file| file.path == format!("{package_path}/__init__.py"))
        {
            return Err(RepositoryError::Invalid);
        }
        let total = files.iter().chain(&docs).try_fold(0usize, |total, file| {
            total
                .checked_add(file.content.len())
                .ok_or(RepositoryError::Invalid)
        })?;
        if total > MAX_LIBRARY_CONTENT_BYTES
            || files
                .iter()
                .any(|file| has_forbidden_import(&file.content, &name))
        {
            return Err(RepositoryError::Invalid);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        docs.sort_by(|left, right| left.path.cmp(&right.path));
        let mut release = Self {
            format: FORMAT.to_owned(),
            name,
            version,
            guidance,
            docs,
            files,
            digest: String::new(),
        };
        release.digest = hex_digest(Sha256::digest(release.canonical_bytes()?));
        Ok(release)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RepositoryError> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| RepositoryError::Corrupt)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_LIBRARY_BUNDLE_BYTES {
            return Err(RepositoryError::Invalid);
        }
        Ok(bytes)
    }

    fn validate_stored(
        self,
        name: &str,
        version: &Version,
        bytes: &[u8],
    ) -> Result<Self, RepositoryError> {
        let canonical = Self::new(
            self.name,
            self.version,
            self.files,
            self.guidance,
            self.docs,
        )
        .map_err(|_| RepositoryError::Corrupt)?;
        if canonical.name != name
            || &canonical.version != version
            || canonical
                .canonical_bytes()
                .map_err(|_| RepositoryError::Corrupt)?
                != bytes
        {
            return Err(RepositoryError::Corrupt);
        }
        Ok(canonical)
    }
}

#[must_use]
pub fn package_namespace(name: &str) -> String {
    format!("faktory_shared.{name}")
}

fn normalize_release_files(
    files: Vec<ProjectFile>,
    prefix: &str,
    python_only: bool,
) -> Result<Vec<ProjectFile>, RepositoryError> {
    let mut paths = BTreeSet::new();
    let mut normalized = Vec::with_capacity(files.len());
    for file in files {
        let path = normalize_user_path(&file.path)?;
        let content = normalize_text(&file.content);
        if !path.starts_with(prefix)
            || content.len() > MAX_LIBRARY_FILE_BYTES
            || (python_only
                && std::path::Path::new(&path).extension() != Some(std::ffi::OsStr::new("py")))
            || !paths.insert(path.clone())
        {
            return Err(RepositoryError::Invalid);
        }
        normalized.push(ProjectFile { path, content });
    }
    Ok(normalized)
}

fn has_forbidden_import(source: &str, own_name: &str) -> bool {
    source.lines().any(|line| {
        line.split(';').map(str::trim_start).any(|statement| {
            if let Some(module) = statement
                .strip_prefix("from ")
                .and_then(|rest| rest.split_ascii_whitespace().next())
            {
                return forbidden_shared_module(module, own_name);
            }
            statement.strip_prefix("import ").is_some_and(|imports| {
                imports.split(',').any(|import| {
                    let module = import.trim_start().split_ascii_whitespace().next();
                    module.is_some_and(|module| forbidden_shared_module(module, own_name))
                })
            })
        })
    })
}

fn forbidden_shared_module(module: &str, own_name: &str) -> bool {
    if module == "faktory_shared" {
        return true;
    }
    module.strip_prefix("faktory_shared.").is_some_and(|rest| {
        let name = rest
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .next()
            .unwrap_or_default();
        name != own_name
    })
}

#[must_use]
pub fn library_release_key(name: &str, version: &Version) -> String {
    format!("libraries/{name}/releases/{version}/release.json")
}

#[must_use]
pub fn library_index_key(name: &str) -> String {
    format!("libraries/{name}/index.json")
}

impl Repository {
    pub async fn publish_library(
        &self,
        release: LibraryRelease,
    ) -> Result<LibraryRelease, RepositoryError> {
        let canonical = LibraryRelease::new(
            release.name,
            release.version,
            release.files,
            release.guidance,
            release.docs,
        )?;
        if !release.digest.is_empty() && release.digest != canonical.digest {
            return Err(RepositoryError::Invalid);
        }
        let _guard = self.mutations.lock().await;
        let existing = self.list_library_releases(&canonical.name).await?;
        if let Some(same) = existing
            .iter()
            .find(|item| item.version == canonical.version)
        {
            return if same.digest == canonical.digest {
                self.persist_library_index(&canonical.name, &existing)
                    .await?;
                if existing.iter().any(|item| {
                    item.version != canonical.version
                        && item.version.major == canonical.version.major
                }) {
                    self.create_rollout_intent(same).await?;
                }
                Ok(same.clone())
            } else {
                Err(RepositoryError::Conflict)
            };
        }
        if existing
            .iter()
            .any(|item| item.version >= canonical.version)
        {
            return Err(RepositoryError::Conflict);
        }
        self.put_immutable(
            &library_release_key(&canonical.name, &canonical.version),
            Bytes::from(canonical.canonical_bytes()?),
        )
        .await?;
        let mut indexed = existing.clone();
        indexed.push(canonical.clone());
        self.persist_library_index(&canonical.name, &indexed)
            .await?;
        if existing
            .iter()
            .any(|item| item.version.major == canonical.version.major)
        {
            self.create_rollout_intent(&canonical).await?;
        }
        Ok(canonical)
    }

    pub async fn get_library(
        &self,
        name: &str,
        version: &Version,
    ) -> Result<LibraryRelease, RepositoryError> {
        validate_library_name(name)?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(RepositoryError::Invalid);
        }
        let object = self.store.get(&library_release_key(name, version)).await?;
        if !object.bytes.ends_with(b"\n") {
            return Err(RepositoryError::Corrupt);
        }
        let release = serde_json::from_slice::<LibraryRelease>(&object.bytes)
            .map_err(|_| RepositoryError::Corrupt)?;
        release.validate_stored(name, version, &object.bytes)
    }

    pub async fn list_library_releases(
        &self,
        name: &str,
    ) -> Result<Vec<LibraryRelease>, RepositoryError> {
        validate_library_name(name)?;
        let prefix = format!("libraries/{name}/releases/");
        let mut releases = Vec::new();
        for key in self.store.list(&prefix).await? {
            let Some(version) = key
                .strip_prefix(&prefix)
                .and_then(|suffix| suffix.strip_suffix("/release.json"))
                .and_then(|value| Version::parse(value).ok())
            else {
                continue;
            };
            releases.push(self.get_library(name, &version).await?);
        }
        releases.sort_by(|left, right| left.version.cmp(&right.version));
        Ok(releases)
    }

    pub async fn list_libraries(&self) -> Result<Vec<LibraryCatalogEntry>, RepositoryError> {
        let mut result = Vec::new();
        for key in self.store.list("libraries/").await? {
            let Some(name) = key
                .strip_prefix("libraries/")
                .and_then(|suffix| suffix.strip_suffix("/index.json"))
            else {
                continue;
            };
            validate_library_name(name).map_err(|_| RepositoryError::Corrupt)?;
            let object = self.store.get(&key).await?;
            if !object.bytes.ends_with(b"\n") {
                return Err(RepositoryError::Corrupt);
            }
            let index: LibraryCatalogEntry =
                serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
            let releases = self.list_library_releases(name).await?;
            let canonical = Self::catalog_entry(name, &releases);
            let mut canonical_bytes =
                serde_json::to_vec(&canonical).map_err(|_| RepositoryError::Corrupt)?;
            canonical_bytes.push(b'\n');
            if index != canonical || object.bytes != canonical_bytes {
                return Err(RepositoryError::Corrupt);
            }
            result.push(index);
        }
        result.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(result)
    }

    fn catalog_entry(name: &str, releases: &[LibraryRelease]) -> LibraryCatalogEntry {
        let mut versions = releases
            .iter()
            .map(|release| release.version.clone())
            .collect::<Vec<_>>();
        versions.sort();
        LibraryCatalogEntry {
            name: name.to_owned(),
            package: package_namespace(name),
            versions,
        }
    }

    async fn persist_library_index(
        &self,
        name: &str,
        releases: &[LibraryRelease],
    ) -> Result<(), RepositoryError> {
        let mut bytes = serde_json::to_vec(&Self::catalog_entry(name, releases))
            .map_err(|_| RepositoryError::Corrupt)?;
        bytes.push(b'\n');
        self.store
            .put(&library_index_key(name), bytes.into(), PutCondition::Any)
            .await?;
        Ok(())
    }

    pub async fn resolve_library(
        &self,
        name: &str,
        requirement: &str,
    ) -> Result<LibraryRelease, RepositoryError> {
        let (major, parsed) = parse_requirement(requirement)?;
        self.list_library_releases(name)
            .await?
            .into_iter()
            .filter(|release| release.version.major == major && parsed.matches(&release.version))
            .max_by(|left, right| left.version.cmp(&right.version))
            .ok_or(RepositoryError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryObjectStore;
    use std::sync::Arc;

    fn file(path: String, content: &str) -> ProjectFile {
        ProjectFile {
            path,
            content: content.to_owned(),
        }
    }
    fn release(name: &str, version: &str, body: &str) -> LibraryRelease {
        LibraryRelease::new(
            name.to_owned(),
            Version::parse(version).expect("version"),
            vec![file(format!("faktory_shared/{name}/__init__.py"), body)],
            "Use it".to_owned(),
            vec![file("docs/guide.md".to_owned(), "API")],
        )
        .expect("release")
    }

    #[tokio::test]
    async fn releases_are_immutable_ordered_and_resolve_highest_same_major() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 4);
        for version in ["1.0.0", "1.3.0", "2.0.0"] {
            repository
                .publish_library(release("gears", version, version))
                .await
                .expect("publish");
        }
        repository
            .publish_library(release("gears", "2.0.0", "2.0.0"))
            .await
            .expect("idempotent");
        assert_eq!(
            repository
                .publish_library(release("gears", "2.0.0", "different"))
                .await,
            Err(RepositoryError::Conflict)
        );
        assert_eq!(
            repository
                .publish_library(release("gears", "1.4.0", "late"))
                .await,
            Err(RepositoryError::Conflict)
        );
        let resolved = repository
            .resolve_library("gears", ">=1.0.0,<2.0.0")
            .await
            .expect("resolve");
        assert_eq!(resolved.version, Version::new(1, 3, 0));
        assert_eq!(package_namespace("gears"), "faktory_shared.gears");
        let catalog = repository.list_libraries().await.expect("catalog");
        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog[0].versions,
            vec![
                Version::new(1, 0, 0),
                Version::new(1, 3, 0),
                Version::new(2, 0, 0)
            ]
        );
    }

    #[test]
    fn rejects_invalid_names_versions_namespaces_and_cross_library_imports() {
        for name in ["Bad", "bad-name", "_bad"] {
            assert!(
                LibraryRelease::new(
                    name.to_owned(),
                    Version::new(1, 0, 0),
                    vec![],
                    "guide".to_owned(),
                    vec![]
                )
                .is_err()
            );
        }
        assert!(
            LibraryRelease::new(
                "gears".to_owned(),
                Version::parse("1.0.0-alpha.1").expect("version"),
                vec![file("faktory_shared/gears/__init__.py".to_owned(), "")],
                "guide".to_owned(),
                vec![file("docs/guide.md".to_owned(), "API")]
            )
            .is_err()
        );
        assert!(
            LibraryRelease::new(
                "gears".to_owned(),
                Version::new(1, 0, 0),
                vec![file("faktory_shared/gears/\u{e9}.py".to_owned(), "")],
                "guide".to_owned(),
                vec![file("docs/guide.md".to_owned(), "API")]
            )
            .is_err()
        );
        for body in [
            "from faktory_shared.other import x",
            "from faktory_shared import other",
            "import os, faktory_shared.other as other",
            "import faktory_shared.gears; import faktory_shared.other",
        ] {
            assert!(
                LibraryRelease::new(
                    "gears".to_owned(),
                    Version::new(1, 0, 0),
                    vec![file("faktory_shared/gears/__init__.py".to_owned(), body)],
                    "guide".to_owned(),
                    vec![file("docs/guide.md".to_owned(), "API")]
                )
                .is_err()
            );
        }
    }
}
