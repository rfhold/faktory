//! Immutable model release identities and exact dependency closure validation.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    Repository, RepositoryError, StoredRenderState,
    project::{ModelLock, ProjectBundle, hex_digest, package_namespace, parse_requirement},
    validate_model_id, validate_revision,
};
use crate::storage::PutCondition;

const FORMAT: &str = "faktory-model-release-v1";
const MAX_DEPENDENCIES: usize = 64;
const MAX_DEPTH: usize = 8;
const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRelease {
    pub format: String,
    pub model_id: String,
    pub version: Version,
    pub project_revision: String,
    #[serde(skip)]
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReleaseIndex {
    pub model_id: String,
    pub package: String,
    pub versions: Vec<Version>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureIdentity {
    pub model_id: String,
    pub package: String,
    pub version: Option<Version>,
    pub project_revision: String,
    pub release_sha256: Option<String>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedProject {
    pub identity: ClosureIdentity,
    pub project: ProjectBundle,
}

impl ModelRelease {
    pub fn new(
        model_id: String,
        version: Version,
        project_revision: String,
    ) -> Result<Self, RepositoryError> {
        validate_model_id(&model_id)?;
        validate_revision(&project_revision)?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(RepositoryError::Invalid);
        }
        let mut release = Self {
            format: FORMAT.to_owned(),
            model_id,
            version,
            project_revision,
            digest: String::new(),
        };
        release.digest = hex_digest(Sha256::digest(release.canonical_bytes()?));
        Ok(release)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RepositoryError> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| RepositoryError::Corrupt)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[must_use]
pub fn model_release_key(model_id: &str, version: &Version) -> String {
    format!("models/{model_id}/releases/{version}/release.json")
}

#[must_use]
pub fn model_release_index_key(model_id: &str) -> String {
    format!("models/{model_id}/releases/index.json")
}

impl Repository {
    pub async fn publish_model_release(
        &self,
        model_id: &str,
        version: Version,
        expected_revision: &str,
    ) -> Result<ModelRelease, RepositoryError> {
        let _guard = self.mutations.lock().await;
        let model = self.get_model(model_id).await?.record;
        if model.render_state != StoredRenderState::Ready
            || model.desired_source_revision != expected_revision
            || model.current_successful_source_revision != expected_revision
        {
            return Err(RepositoryError::Conflict);
        }
        let project = self.get_project(model_id, expected_revision).await?;
        if !project
            .files
            .iter()
            .any(|file| file.path == "faktory_model/__init__.py")
        {
            return Err(RepositoryError::Invalid);
        }
        self.resolve_project_closure(model_id, expected_revision)
            .await?;
        let canonical =
            ModelRelease::new(model_id.to_owned(), version, expected_revision.to_owned())?;
        let existing = self.list_model_releases(model_id).await?;
        if let Some(same) = existing
            .iter()
            .find(|item| item.version == canonical.version)
        {
            if same.digest != canonical.digest {
                return Err(RepositoryError::Conflict);
            }
            self.persist_model_release_index(model_id, &existing)
                .await?;
            if existing.iter().any(|item| {
                item.version != canonical.version && item.version.major == canonical.version.major
            }) {
                self.create_rollout_intent(same).await?;
            }
            return Ok(same.clone());
        }
        if existing
            .iter()
            .any(|item| item.version >= canonical.version)
        {
            return Err(RepositoryError::Conflict);
        }
        self.put_immutable(
            &model_release_key(model_id, &canonical.version),
            Bytes::from(canonical.canonical_bytes()?),
        )
        .await?;
        let has_same_major = existing
            .iter()
            .any(|item| item.version.major == canonical.version.major);
        let mut indexed = existing;
        indexed.push(canonical.clone());
        self.persist_model_release_index(model_id, &indexed).await?;
        if has_same_major {
            self.create_rollout_intent(&canonical).await?;
        }
        Ok(canonical)
    }

    pub async fn get_model_release(
        &self,
        model_id: &str,
        version: &Version,
    ) -> Result<ModelRelease, RepositoryError> {
        validate_model_id(model_id)?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(RepositoryError::Invalid);
        }
        let object = self
            .store
            .get(&model_release_key(model_id, version))
            .await?;
        if !object.bytes.ends_with(b"\n") {
            return Err(RepositoryError::Corrupt);
        }
        let stored: ModelRelease =
            serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
        let canonical = ModelRelease::new(stored.model_id, stored.version, stored.project_revision)
            .map_err(|_| RepositoryError::Corrupt)?;
        if canonical.model_id != model_id
            || &canonical.version != version
            || canonical.canonical_bytes()? != object.bytes
        {
            return Err(RepositoryError::Corrupt);
        }
        Ok(canonical)
    }

    pub async fn list_model_releases(
        &self,
        model_id: &str,
    ) -> Result<Vec<ModelRelease>, RepositoryError> {
        validate_model_id(model_id)?;
        let prefix = format!("models/{model_id}/releases/");
        let mut releases = Vec::new();
        for key in self.store.list(&prefix).await? {
            let Some(version) = key
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix("/release.json"))
                .and_then(|value| Version::parse(value).ok())
            else {
                continue;
            };
            releases.push(self.get_model_release(model_id, &version).await?);
        }
        releases.sort_by(|left, right| left.version.cmp(&right.version));
        Ok(releases)
    }

    async fn persist_model_release_index(
        &self,
        model_id: &str,
        releases: &[ModelRelease],
    ) -> Result<(), RepositoryError> {
        let index = ModelReleaseIndex {
            model_id: model_id.to_owned(),
            package: package_namespace(model_id),
            versions: releases
                .iter()
                .map(|release| release.version.clone())
                .collect(),
        };
        let mut bytes = serde_json::to_vec(&index).map_err(|_| RepositoryError::Corrupt)?;
        bytes.push(b'\n');
        self.store
            .put(
                &model_release_index_key(model_id),
                bytes.into(),
                PutCondition::Any,
            )
            .await?;
        Ok(())
    }

    pub async fn resolve_model_release(
        &self,
        model_id: &str,
        requirement: &str,
    ) -> Result<ModelRelease, RepositoryError> {
        let (major, parsed) = parse_requirement(requirement)?;
        self.list_model_releases(model_id)
            .await?
            .into_iter()
            .filter(|release| release.version.major == major && parsed.matches(&release.version))
            .max_by(|left, right| left.version.cmp(&right.version))
            .ok_or(RepositoryError::NotFound)
    }

    pub async fn resolve_project_closure(
        &self,
        root_model_id: &str,
        root_revision: &str,
    ) -> Result<Vec<ResolvedProject>, RepositoryError> {
        validate_model_id(root_model_id)?;
        validate_revision(root_revision)?;
        let root = self.get_project(root_model_id, root_revision).await?;
        self.resolve_closure_from_root(root_model_id, root_revision, root)
            .await
    }

    pub(crate) async fn validate_project_closure(
        &self,
        root_model_id: &str,
        root: ProjectBundle,
    ) -> Result<(), RepositoryError> {
        validate_model_id(root_model_id)?;
        let root_revision = root.digest()?;
        self.resolve_closure_from_root(root_model_id, &root_revision, root)
            .await
            .map(|_| ())
    }

    #[allow(clippy::too_many_lines)]
    async fn resolve_closure_from_root(
        &self,
        root_model_id: &str,
        root_revision: &str,
        root: ProjectBundle,
    ) -> Result<Vec<ResolvedProject>, RepositoryError> {
        let mut resolved = BTreeMap::new();
        let mut pending = vec![(
            root_model_id.to_owned(),
            root_revision.to_owned(),
            None,
            None,
            root,
            0usize,
        )];
        let mut package_bytes = 0usize;
        while let Some((model_id, revision, version, release_sha256, project, depth)) =
            pending.pop()
        {
            if depth > MAX_DEPTH || (depth > 0 && model_id == root_model_id) {
                return Err(RepositoryError::Invalid);
            }
            if let Some(existing) = resolved.get(&model_id) {
                let existing: &ResolvedProject = existing;
                if existing.identity.project_revision != revision
                    || existing.identity.version != version
                    || existing.identity.release_sha256 != release_sha256
                {
                    return Err(RepositoryError::Invalid);
                }
                continue;
            }
            if depth > 0 && resolved.len() > MAX_DEPENDENCIES {
                return Err(RepositoryError::Invalid);
            }
            let declared = project
                .locks
                .iter()
                .map(|lock| lock.model_id.clone())
                .collect::<BTreeSet<_>>();
            validate_import_edges(&project, &model_id, &declared)?;
            if depth > 0 {
                let source_bytes = project
                    .files
                    .iter()
                    .filter(|file| {
                        file.path == "faktory_model/__init__.py"
                            || file.path.starts_with("faktory_model/")
                    })
                    .map(|file| file.content.len())
                    .sum::<usize>();
                package_bytes = checked_package_bytes(package_bytes, source_bytes)?;
            }
            let dependencies = project
                .locks
                .iter()
                .map(|lock| lock.model_id.clone())
                .collect();
            for lock in project.locks.iter().rev() {
                self.validated_locked_release(lock).await?;
                let child = self
                    .get_project(&lock.model_id, &lock.project_revision)
                    .await?;
                pending.push((
                    lock.model_id.clone(),
                    lock.project_revision.clone(),
                    Some(lock.version.clone()),
                    Some(lock.release_sha256.clone()),
                    child,
                    depth + 1,
                ));
            }
            resolved.insert(
                model_id.clone(),
                ResolvedProject {
                    identity: ClosureIdentity {
                        model_id: model_id.clone(),
                        package: package_namespace(&model_id),
                        version,
                        project_revision: revision,
                        release_sha256,
                        dependencies,
                    },
                    project,
                },
            );
        }
        validate_graph(root_model_id, &resolved)?;
        Ok(resolved.into_values().collect())
    }

    #[allow(clippy::suspicious_operation_groupings)]
    async fn validated_locked_release(
        &self,
        lock: &ModelLock,
    ) -> Result<ModelRelease, RepositoryError> {
        let release = self
            .get_model_release(&lock.model_id, &lock.version)
            .await?;
        if (release.project_revision != lock.project_revision)
            || (release.digest != lock.release_sha256)
        {
            return Err(RepositoryError::Corrupt);
        }
        Ok(release)
    }
}

fn validate_import_edges(
    project: &ProjectBundle,
    own_model_id: &str,
    declared: &BTreeSet<String>,
) -> Result<(), RepositoryError> {
    let mut allowed = declared
        .iter()
        .map(|model_id| package_namespace(model_id))
        .collect::<BTreeSet<_>>();
    allowed.insert(package_namespace(own_model_id));
    for file in project.files.iter().filter(|file| {
        std::path::Path::new(&file.path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("py"))
    }) {
        validate_python_imports(&file.content, &allowed)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PythonToken {
    Identifier(String),
    Dot,
    Comma,
    Star,
    Open,
    Close,
    Boundary,
}

fn validate_python_imports(
    source: &str,
    allowed_packages: &BTreeSet<String>,
) -> Result<(), RepositoryError> {
    let tokens = tokenize_python(source)?;
    let mut index = 0;
    let mut statement_start = true;
    while index < tokens.len() {
        match &tokens[index] {
            PythonToken::Boundary => {
                statement_start = true;
                index += 1;
            }
            PythonToken::Identifier(keyword) if statement_start && keyword == "import" => {
                index += 1;
                loop {
                    let target = parse_dotted_name(&tokens, &mut index)?;
                    validate_import_target(&target, allowed_packages)?;
                    skip_alias(&tokens, &mut index)?;
                    match tokens.get(index) {
                        Some(PythonToken::Comma) => index += 1,
                        Some(PythonToken::Boundary) | None => break,
                        _ => return Err(RepositoryError::Invalid),
                    }
                }
                statement_start = false;
            }
            PythonToken::Identifier(keyword) if statement_start && keyword == "from" => {
                index += 1;
                let mut relative = false;
                while matches!(tokens.get(index), Some(PythonToken::Dot)) {
                    relative = true;
                    index += 1;
                }
                let target = if relative
                    && matches!(tokens.get(index), Some(PythonToken::Identifier(value)) if value == "import")
                {
                    String::new()
                } else {
                    parse_dotted_name(&tokens, &mut index)?
                };
                if !matches!(tokens.get(index), Some(PythonToken::Identifier(value)) if value == "import")
                {
                    return Err(RepositoryError::Invalid);
                }
                index += 1;
                if relative {
                    skip_import_names(&tokens, &mut index)?;
                } else if target == "faktory_models" {
                    validate_model_import_names(&tokens, &mut index, allowed_packages)?;
                } else {
                    validate_import_target(&target, allowed_packages)?;
                    skip_import_names(&tokens, &mut index)?;
                }
                statement_start = false;
            }
            _ => {
                statement_start = false;
                index += 1;
            }
        }
    }
    Ok(())
}

fn parse_dotted_name(tokens: &[PythonToken], index: &mut usize) -> Result<String, RepositoryError> {
    let Some(PythonToken::Identifier(first)) = tokens.get(*index) else {
        return Err(RepositoryError::Invalid);
    };
    let mut name = first.clone();
    *index += 1;
    while matches!(tokens.get(*index), Some(PythonToken::Dot)) {
        *index += 1;
        let Some(PythonToken::Identifier(component)) = tokens.get(*index) else {
            return Err(RepositoryError::Invalid);
        };
        name.push('.');
        name.push_str(component);
        *index += 1;
    }
    Ok(name)
}

fn validate_import_target(
    target: &str,
    allowed_packages: &BTreeSet<String>,
) -> Result<(), RepositoryError> {
    if target == "faktory_shared"
        || target.starts_with("faktory_shared.")
        || target == "faktory_model"
        || target.starts_with("faktory_model.")
    {
        return Err(RepositoryError::Invalid);
    }
    if target == "faktory_models" {
        return Err(RepositoryError::Invalid);
    }
    if let Some(suffix) = target.strip_prefix("faktory_models.") {
        let package = format!(
            "faktory_models.{}",
            suffix.split('.').next().unwrap_or_default()
        );
        if !allowed_packages.contains(&package) {
            return Err(RepositoryError::Invalid);
        }
    }
    Ok(())
}

fn validate_model_import_names(
    tokens: &[PythonToken],
    index: &mut usize,
    allowed_packages: &BTreeSet<String>,
) -> Result<(), RepositoryError> {
    let parenthesized = matches!(tokens.get(*index), Some(PythonToken::Open));
    if parenthesized {
        *index += 1;
    }
    loop {
        match tokens.get(*index) {
            Some(PythonToken::Close) if parenthesized => {
                *index += 1;
                return Ok(());
            }
            Some(PythonToken::Identifier(name)) => {
                if !allowed_packages.contains(&format!("faktory_models.{name}")) {
                    return Err(RepositoryError::Invalid);
                }
                *index += 1;
                skip_alias(tokens, index)?;
            }
            _ => return Err(RepositoryError::Invalid),
        }
        match tokens.get(*index) {
            Some(PythonToken::Comma) => *index += 1,
            Some(PythonToken::Close) if parenthesized => {
                *index += 1;
                return Ok(());
            }
            Some(PythonToken::Boundary) | None if !parenthesized => return Ok(()),
            _ => return Err(RepositoryError::Invalid),
        }
    }
}

fn skip_import_names(tokens: &[PythonToken], index: &mut usize) -> Result<(), RepositoryError> {
    let parenthesized = matches!(tokens.get(*index), Some(PythonToken::Open));
    if parenthesized {
        *index += 1;
    }
    loop {
        match tokens.get(*index) {
            Some(PythonToken::Identifier(_) | PythonToken::Star) => *index += 1,
            Some(PythonToken::Close) if parenthesized => {
                *index += 1;
                return Ok(());
            }
            _ => return Err(RepositoryError::Invalid),
        }
        skip_alias(tokens, index)?;
        match tokens.get(*index) {
            Some(PythonToken::Comma) => *index += 1,
            Some(PythonToken::Close) if parenthesized => {
                *index += 1;
                return Ok(());
            }
            Some(PythonToken::Boundary) | None if !parenthesized => return Ok(()),
            _ => return Err(RepositoryError::Invalid),
        }
    }
}

fn skip_alias(tokens: &[PythonToken], index: &mut usize) -> Result<(), RepositoryError> {
    if matches!(tokens.get(*index), Some(PythonToken::Identifier(value)) if value == "as") {
        *index += 1;
        if !matches!(tokens.get(*index), Some(PythonToken::Identifier(_))) {
            return Err(RepositoryError::Invalid);
        }
        *index += 1;
    }
    Ok(())
}

fn tokenize_python(source: &str) -> Result<Vec<PythonToken>, RepositoryError> {
    let characters = source.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut nesting = 0usize;
    while index < characters.len() {
        let character = characters[index];
        if character == '#' {
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
        } else if character == '\'' || character == '"' {
            let quote = character;
            let triple = characters.get(index + 1) == Some(&quote)
                && characters.get(index + 2) == Some(&quote);
            index += if triple { 3 } else { 1 };
            let mut closed = false;
            while index < characters.len() {
                if characters[index] == '\\' {
                    index = (index + 2).min(characters.len());
                } else if characters[index] == quote
                    && (!triple
                        || (characters.get(index + 1) == Some(&quote)
                            && characters.get(index + 2) == Some(&quote)))
                {
                    index += if triple { 3 } else { 1 };
                    closed = true;
                    break;
                } else if !triple && characters[index] == '\n' {
                    return Err(RepositoryError::Invalid);
                } else {
                    index += 1;
                }
            }
            if !closed {
                return Err(RepositoryError::Invalid);
            }
        } else if character == '\\' && characters.get(index + 1) == Some(&'\n') {
            index += 2;
        } else if character == '\n' || character == ';' || character == ':' {
            if nesting == 0 {
                tokens.push(PythonToken::Boundary);
            }
            index += 1;
        } else if character == '(' || character == '[' || character == '{' {
            nesting += 1;
            tokens.push(PythonToken::Open);
            index += 1;
        } else if character == ')' || character == ']' || character == '}' {
            nesting = nesting.checked_sub(1).ok_or(RepositoryError::Invalid)?;
            tokens.push(PythonToken::Close);
            index += 1;
        } else if character == '.' {
            tokens.push(PythonToken::Dot);
            index += 1;
        } else if character == ',' {
            tokens.push(PythonToken::Comma);
            index += 1;
        } else if character == '*' {
            tokens.push(PythonToken::Star);
            index += 1;
        } else if character == '_' || character.is_alphabetic() {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index] == '_' || characters[index].is_alphanumeric())
            {
                index += 1;
            }
            tokens.push(PythonToken::Identifier(
                characters[start..index].iter().collect(),
            ));
        } else {
            index += 1;
        }
    }
    if nesting != 0 {
        return Err(RepositoryError::Invalid);
    }
    Ok(tokens)
}

fn checked_package_bytes(current: usize, additional: usize) -> Result<usize, RepositoryError> {
    let total = current
        .checked_add(additional)
        .ok_or(RepositoryError::Invalid)?;
    if total > MAX_PACKAGE_BYTES {
        return Err(RepositoryError::Invalid);
    }
    Ok(total)
}

fn validate_graph(
    root_model_id: &str,
    resolved: &BTreeMap<String, ResolvedProject>,
) -> Result<(), RepositoryError> {
    fn visit(
        model_id: &str,
        resolved: &BTreeMap<String, ResolvedProject>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<(), RepositoryError> {
        if depth > MAX_DEPTH || !visiting.insert(model_id.to_owned()) {
            return Err(RepositoryError::Invalid);
        }
        if visited.contains(model_id) {
            visiting.remove(model_id);
            return Ok(());
        }
        let node = resolved.get(model_id).ok_or(RepositoryError::Invalid)?;
        for dependency in &node.identity.dependencies {
            visit(dependency, resolved, visiting, visited, depth + 1)?;
        }
        visiting.remove(model_id);
        visited.insert(model_id.to_owned());
        Ok(())
    }
    if resolved.len() > MAX_DEPENDENCIES + 1 {
        return Err(RepositoryError::Invalid);
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    visit(root_model_id, resolved, &mut visiting, &mut visited, 0)?;
    if visited.len() != resolved.len() {
        return Err(RepositoryError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bytes::Bytes;

    use super::*;
    use crate::{
        model::{
            GeometryFactsRecord, GeometrySizeRecord, ModelOutputSummaryRecord, OutputManifest,
            OutputRoleRecord, RenderedModelOutput, RenderedOutput, TechnicalProjectionImages,
            project::{DirectRequirement, ProjectEdit, ProjectFile, ProjectOperation},
        },
        storage::{InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject},
    };

    #[derive(Debug, Default)]
    struct FailPutOnceStore {
        inner: InMemoryObjectStore,
        fail_key: Mutex<Option<String>>,
    }

    impl FailPutOnceStore {
        fn arm(&self, key: String) {
            *self.fail_key.lock().expect("failpoint lock") = Some(key);
        }
    }

    #[async_trait]
    impl ObjectStore for FailPutOnceStore {
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
            let should_fail = self
                .fail_key
                .lock()
                .expect("failpoint lock")
                .as_ref()
                .is_some_and(|fail_key| fail_key == key);
            if should_fail {
                self.fail_key.lock().expect("failpoint lock").take();
                return Err(StorageError::Unavailable);
            }
            self.inner.put(key, bytes, condition).await
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    fn file(path: &str, content: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            content: content.to_owned(),
        }
    }

    fn rendered() -> RenderedOutput {
        let image = Bytes::from(
            resvg::tiny_skia::Pixmap::new(
                crate::render::PROJECTION_WIDTH,
                crate::render::PROJECTION_HEIGHT,
            )
            .expect("projection")
            .encode_png()
            .expect("PNG"),
        );
        let summary = ModelOutputSummaryRecord {
            output_id: "primary".to_owned(),
            role: OutputRoleRecord::Part,
            primary: true,
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            },
        };
        RenderedOutput {
            manifest: OutputManifest {
                format: OutputManifest::FORMAT.to_owned(),
                outputs: vec![summary.clone()],
            }
            .canonical_bytes()
            .expect("manifest"),
            outputs: vec![RenderedModelOutput {
                summary,
                glb: Bytes::from_static(b"glb"),
                preview: Bytes::from_static(b"<svg></svg>"),
                projections: TechnicalProjectionImages::all(image.clone()),
                shaded: Some(TechnicalProjectionImages::all(image)),
            }],
        }
    }

    async fn complete(repository: &Repository, model_id: &str, revision: &str) {
        repository
            .complete_render(model_id, revision, rendered())
            .await
            .expect("complete render");
    }

    #[allow(clippy::too_many_lines)]
    async fn publication_failure_fixture() -> (Arc<FailPutOnceStore>, String, String) {
        let store = Arc::new(FailPutOnceStore::default());
        let repository = Repository::new(store.clone(), 1);
        let provider = repository
            .create_project_from_files(
                "failure-provider",
                "Failure Provider",
                vec![
                    file("main.py", "result = None\n"),
                    file("faktory_model/__init__.py", "VALUE = 1\n"),
                ],
                "main.py".to_owned(),
                vec![],
                "",
            )
            .await
            .expect("provider");
        complete(
            &repository,
            "failure-provider",
            &provider.desired_source_revision,
        )
        .await;
        repository
            .publish_model_release(
                "failure-provider",
                Version::new(1, 0, 0),
                &provider.desired_source_revision,
            )
            .await
            .expect("first release");
        let consumer = repository
            .create_project_from_files(
                "failure-consumer",
                "Failure Consumer",
                vec![file("main.py", "result = None\n")],
                "main.py".to_owned(),
                vec![DirectRequirement {
                    model_id: "failure-provider".to_owned(),
                    range: ">=1.0.0,<2.0.0".to_owned(),
                }],
                "",
            )
            .await
            .expect("consumer");
        complete(
            &repository,
            "failure-consumer",
            &consumer.desired_source_revision,
        )
        .await;
        let edit = ProjectEdit::new(vec![ProjectOperation::FilePatch {
            path: "faktory_model/__init__.py".to_owned(),
            patches: vec![crate::model::project::ExactPatch {
                old: "VALUE = 1".to_owned(),
                new: "VALUE = 2".to_owned(),
            }],
        }])
        .expect("provider edit");
        let next = repository
            .edit_project(
                "failure-provider",
                &provider.desired_source_revision,
                None,
                Some(&edit),
            )
            .await
            .expect("edit provider")
            .record;
        complete(
            &repository,
            "failure-provider",
            &next.desired_source_revision,
        )
        .await;
        (
            store,
            next.desired_source_revision,
            consumer.desired_source_revision,
        )
    }

    #[test]
    fn release_identity_is_exact_canonical_compact_json() {
        let release =
            ModelRelease::new("gear-box".to_owned(), Version::new(1, 2, 3), "a".repeat(64))
                .expect("release");
        assert_eq!(
            release.canonical_bytes().expect("canonical"),
            format!("{{\"format\":\"faktory-model-release-v1\",\"model_id\":\"gear-box\",\"version\":\"1.2.3\",\"project_revision\":\"{}\"}}\n", "a".repeat(64)).into_bytes()
        );
        assert_eq!(
            model_release_key("gear-box", &Version::new(1, 2, 3)),
            "models/gear-box/releases/1.2.3/release.json"
        );
    }

    #[tokio::test]
    async fn publication_recovers_release_index_and_intent_write_failures() {
        for boundary in ["release", "index", "intent"] {
            let (store, provider_revision, consumer_revision) = publication_failure_fixture().await;
            let expected = ModelRelease::new(
                "failure-provider".to_owned(),
                Version::new(1, 1, 0),
                provider_revision.clone(),
            )
            .expect("expected release");
            let failure_key = match boundary {
                "release" => model_release_key("failure-provider", &expected.version),
                "index" => model_release_index_key("failure-provider"),
                "intent" => crate::model::rollout::rollout_key(
                    "failure-provider",
                    &expected.version,
                    &expected.digest,
                ),
                _ => unreachable!(),
            };
            store.arm(failure_key);
            let repository = Repository::new(store.clone(), 1);
            assert_eq!(
                repository
                    .publish_model_release(
                        "failure-provider",
                        expected.version.clone(),
                        &provider_revision,
                    )
                    .await,
                Err(RepositoryError::Unavailable),
                "boundary {boundary}"
            );

            let restarted = Repository::new(store.clone(), 1);
            let recovered = restarted
                .publish_model_release(
                    "failure-provider",
                    expected.version.clone(),
                    &provider_revision,
                )
                .await
                .expect("restart publication");
            assert_eq!(recovered, expected, "boundary {boundary}");
            restarted
                .reconcile_model_release_rollout_intents()
                .await
                .expect("reconcile intents");
            let resumed = restarted
                .resume_incomplete_model_release_rollouts()
                .await
                .expect("resume rollout");
            assert_eq!(resumed.len(), 1, "boundary {boundary}");
            assert!(resumed[0].complete, "boundary {boundary}");
            let releases = restarted
                .list_model_releases("failure-provider")
                .await
                .expect("release index");
            assert_eq!(releases.last(), Some(&expected), "boundary {boundary}");
            let consumer = restarted
                .get_model("failure-consumer")
                .await
                .expect("consumer")
                .record;
            assert_eq!(
                consumer.current_successful_source_revision, consumer_revision,
                "boundary {boundary}"
            );
            let project = restarted
                .get_project("failure-consumer", &consumer.desired_source_revision)
                .await
                .expect("consumer project");
            assert_eq!(
                project.locks[0].version, expected.version,
                "boundary {boundary}"
            );
            assert_eq!(
                project.locks[0].release_sha256, expected.digest,
                "boundary {boundary}"
            );
        }
    }

    fn graph_node(model_id: &str, dependencies: Vec<String>) -> ResolvedProject {
        ResolvedProject {
            identity: ClosureIdentity {
                model_id: model_id.to_owned(),
                package: package_namespace(model_id),
                version: None,
                project_revision: "0".repeat(64),
                release_sha256: None,
                dependencies,
            },
            project: ProjectBundle {
                format: "faktory-project-v2".to_owned(),
                entrypoint: "main.py".to_owned(),
                requirements: vec![],
                locks: vec![],
                files: vec![],
            },
        }
    }

    #[test]
    fn graph_count_depth_and_package_byte_boundaries_are_exact() {
        let mut exact_count = BTreeMap::new();
        let dependencies = (0..MAX_DEPENDENCIES)
            .map(|index| format!("dependency-{index}"))
            .collect::<Vec<_>>();
        exact_count.insert("root".to_owned(), graph_node("root", dependencies.clone()));
        for dependency in &dependencies {
            exact_count.insert(dependency.clone(), graph_node(dependency, vec![]));
        }
        assert!(validate_graph("root", &exact_count).is_ok());
        exact_count.insert(
            "dependency-extra".to_owned(),
            graph_node("dependency-extra", vec![]),
        );
        assert_eq!(
            validate_graph("root", &exact_count),
            Err(RepositoryError::Invalid)
        );

        let chain = |edges: usize| {
            let mut graph = BTreeMap::new();
            for index in 0..=edges {
                let model_id = format!("node-{index}");
                let dependencies = if index < edges {
                    vec![format!("node-{}", index + 1)]
                } else {
                    vec![]
                };
                graph.insert(model_id.clone(), graph_node(&model_id, dependencies));
            }
            graph
        };
        assert!(validate_graph("node-0", &chain(MAX_DEPTH)).is_ok());
        assert_eq!(
            validate_graph("node-0", &chain(MAX_DEPTH + 1)),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            checked_package_bytes(0, MAX_PACKAGE_BYTES),
            Ok(MAX_PACKAGE_BYTES)
        );
        assert_eq!(
            checked_package_bytes(MAX_PACKAGE_BYTES, 1),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            checked_package_bytes(usize::MAX, 1),
            Err(RepositoryError::Invalid)
        );
    }

    #[test]
    fn static_python_imports_enforce_own_and_direct_model_edges() {
        let declared = BTreeSet::from(["provider".to_owned()]);
        let project = |content: &str| ProjectBundle {
            format: "faktory-project-v2".to_owned(),
            entrypoint: "main.py".to_owned(),
            requirements: vec![],
            locks: vec![],
            files: vec![file("main.py", content)],
        };
        let accepted = [
            "import faktory_models.m_provider as provider\n",
            "import faktory_models.m_provider as provider, \\\n+    faktory_models.m_root.helpers\n",
            "from faktory_models import (m_provider as provider, m_root)\n",
            "from faktory_models.m_provider.widgets import make as build\n",
            "from . import helpers\nfrom ..shared import VALUE\n",
            "# import faktory_models.m_undeclared\nTEXT = 'from faktory_models.m_undeclared import bad'\nDOC = \"\"\"import faktory_shared.bad\"\"\"\n",
        ];
        for source in accepted {
            assert!(
                validate_import_edges(&project(source), "root", &declared).is_ok(),
                "rejected {source:?}"
            );
        }
        let rejected = [
            "import faktory_models.m_undeclared\n",
            "from faktory_models.m_transitive import VALUE\n",
            "from faktory_models import *\n",
            "import faktory_shared.legacy\n",
            "from faktory_model import private\n",
            "from faktory_models.m_provider import (\n",
        ];
        for source in rejected {
            assert_eq!(
                validate_import_edges(&project(source), "root", &declared),
                Err(RepositoryError::Invalid),
                "accepted {source:?}"
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn same_major_rollout_updates_exact_lock_and_major_adoption_is_explicit() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let provider = repository
            .create_project_from_files(
                "provider",
                "Provider",
                vec![
                    file("main.py", "result = None\n"),
                    file("faktory_model/__init__.py", "VALUE = 1\n"),
                ],
                "main.py".to_owned(),
                vec![],
                "",
            )
            .await
            .expect("provider");
        complete(&repository, "provider", &provider.desired_source_revision).await;
        let first = repository
            .publish_model_release(
                "provider",
                Version::new(1, 0, 0),
                &provider.desired_source_revision,
            )
            .await
            .expect("first release");

        let consumer = repository
            .create_project_from_files(
                "consumer",
                "Consumer",
                vec![file(
                    "main.py",
                    "from faktory_models.m_provider import VALUE\nresult = VALUE\n",
                )],
                "main.py".to_owned(),
                vec![DirectRequirement {
                    model_id: "provider".to_owned(),
                    range: ">=1.0.0,<2.0.0".to_owned(),
                }],
                "",
            )
            .await
            .expect("consumer");
        complete(&repository, "consumer", &consumer.desired_source_revision).await;
        let racing = repository
            .create_project_from_files(
                "racing-consumer",
                "Racing Consumer",
                vec![file("main.py", "result = None\n")],
                "main.py".to_owned(),
                vec![DirectRequirement {
                    model_id: "provider".to_owned(),
                    range: ">=1.0.0,<2.0.0".to_owned(),
                }],
                "",
            )
            .await
            .expect("racing consumer");
        let observed = repository
            .get_project("racing-consumer", &racing.desired_source_revision)
            .await
            .expect("earlier eligible observation");
        assert_eq!(observed.locks[0].version, Version::new(1, 0, 0));

        let provider_edit = ProjectEdit::new(vec![ProjectOperation::FilePatch {
            path: "faktory_model/__init__.py".to_owned(),
            patches: vec![crate::model::project::ExactPatch {
                old: "VALUE = 1".to_owned(),
                new: "VALUE = 2".to_owned(),
            }],
        }])
        .expect("edit");
        let provider_next = repository
            .edit_project(
                "provider",
                &provider.desired_source_revision,
                None,
                Some(&provider_edit),
            )
            .await
            .expect("edit provider")
            .record;
        complete(
            &repository,
            "provider",
            &provider_next.desired_source_revision,
        )
        .await;
        let second = repository
            .publish_model_release(
                "provider",
                Version::new(1, 1, 0),
                &provider_next.desired_source_revision,
            )
            .await
            .expect("second release");
        let mutation_guard = repository.mutations.lock().await;
        let race_repository = repository.clone();
        let race_release = second.clone();
        let race = tokio::spawn(async move {
            race_repository
                .apply_model_release_to_consumer("racing-consumer", &race_release)
                .await
        });
        tokio::task::yield_now().await;
        let remove_dependency = ProjectEdit::new(vec![ProjectOperation::DependenciesSet {
            requirements: vec![],
        }])
        .expect("remove dependency");
        repository
            .edit_project_locked(
                "racing-consumer",
                &racing.desired_source_revision,
                None,
                Some(&remove_dependency),
            )
            .await
            .expect("concurrent consumer mutation");
        drop(mutation_guard);
        assert_eq!(
            race.await.expect("race task").expect("atomic rollout"),
            crate::model::rollout::RolloutModelState::NotEligible
        );
        let intent_key =
            crate::model::rollout::rollout_key(&second.model_id, &second.version, &second.digest);
        let intent = repository.store.get(&intent_key).await.expect("intent");
        repository
            .store
            .delete(&intent_key, &intent.etag)
            .await
            .expect("simulate crash before intent write");
        let reconstructed = repository
            .reconcile_model_release_rollout_intents()
            .await
            .expect("reconstruct intent");
        assert_eq!(reconstructed.len(), 1);
        let repeated = repository
            .reconcile_model_release_rollout_intents()
            .await
            .expect("idempotent reconstruction");
        assert_eq!(repeated, reconstructed);
        let resumed = repository
            .resume_incomplete_model_release_rollouts()
            .await
            .expect("resume rollout");
        assert_eq!(resumed.len(), 1);
        let rollout = &resumed[0];
        assert!(rollout.complete);
        assert_eq!(
            rollout.models.get("racing-consumer"),
            Some(&crate::model::rollout::RolloutModelState::NotEligible)
        );
        let updated = repository
            .get_model("consumer")
            .await
            .expect("consumer")
            .record;
        assert_eq!(
            updated.current_successful_source_revision,
            consumer.desired_source_revision
        );
        let project = repository
            .get_project("consumer", &updated.desired_source_revision)
            .await
            .expect("project");
        assert_eq!(project.locks[0].version, Version::new(1, 1, 0));
        assert_eq!(project.locks[0].project_revision, second.project_revision);
        assert_ne!(project.locks[0].release_sha256, first.digest);

        let provider_major = repository
            .edit_project(
                "provider",
                &provider_next.desired_source_revision,
                None,
                Some(
                    &ProjectEdit::new(vec![ProjectOperation::FilePatch {
                        path: "faktory_model/__init__.py".to_owned(),
                        patches: vec![crate::model::project::ExactPatch {
                            old: "VALUE = 2".to_owned(),
                            new: "VALUE = 3".to_owned(),
                        }],
                    }])
                    .expect("major edit"),
                ),
            )
            .await
            .expect("edit major")
            .record;
        complete(
            &repository,
            "provider",
            &provider_major.desired_source_revision,
        )
        .await;
        repository
            .publish_model_release(
                "provider",
                Version::new(2, 0, 0),
                &provider_major.desired_source_revision,
            )
            .await
            .expect("major release");
        let unchanged = repository
            .get_model("consumer")
            .await
            .expect("consumer")
            .record;
        assert_eq!(
            unchanged.desired_source_revision,
            updated.desired_source_revision
        );

        let adopt = ProjectEdit::new(vec![ProjectOperation::DependenciesSet {
            requirements: vec![DirectRequirement {
                model_id: "provider".to_owned(),
                range: ">=2.0.0,<3.0.0".to_owned(),
            }],
        }])
        .expect("adopt edit");
        let adopted = repository
            .edit_project(
                "consumer",
                &unchanged.desired_source_revision,
                None,
                Some(&adopt),
            )
            .await
            .expect("adopt major")
            .record;
        assert_eq!(
            repository
                .get_project("consumer", &adopted.desired_source_revision)
                .await
                .expect("adopted project")
                .locks[0]
                .version,
            Version::new(2, 0, 0)
        );
    }
}
