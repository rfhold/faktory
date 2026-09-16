//! Canonical multi-file model projects.

use std::{collections::BTreeSet, fmt::Write as _};

use bytes::Bytes;
use semver::{Op, Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    EditedModel, ModelRecord, Repository, RepositoryError, StoredRenderState, mutation_timestamp,
    validate_model_id, validate_name, validate_revision,
};
use crate::storage::PutCondition;

pub const MAX_PROJECT_FILES: usize = 256;
pub const MAX_PROJECT_FILE_BYTES: usize = 1_048_576;
pub const MAX_CALLER_CONTENT_BYTES: usize = 1_048_576;
pub const MAX_CANONICAL_BUNDLE_BYTES: usize = 16_777_216;
pub const MAX_PROJECT_PATH_BYTES: usize = 1_024;
pub const MAX_PROJECT_COMPONENT_BYTES: usize = 255;
pub const MAX_PROJECT_REQUIREMENTS: usize = 64;
pub const AGENTS_PATH: &str = "AGENTS.md";
const FORMAT: &str = "faktory-project-v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectRequirement {
    pub name: String,
    pub range: String,
}

impl DirectRequirement {
    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        parse_requirement(&self.range).is_ok_and(|requirement| requirement.1.matches(version))
    }

    pub fn major(&self) -> Result<u64, RepositoryError> {
        Ok(parse_requirement(&self.range)?.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryLock {
    pub name: String,
    pub version: Version,
    pub release_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyGuidance {
    pub name: String,
    pub version: Version,
    pub release_sha256: String,
    pub guidance: String,
    pub documentation: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBundle {
    pub format: String,
    pub entrypoint: String,
    pub requirements: Vec<DirectRequirement>,
    pub locks: Vec<LibraryLock>,
    pub files: Vec<ProjectFile>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectEdit {
    pub operations: Vec<ProjectOperation>,
    #[doc(hidden)]
    pub(crate) add_files: Vec<ProjectFile>,
    #[doc(hidden)]
    pub(crate) delete_files: Vec<String>,
    #[doc(hidden)]
    pub(crate) renames: Vec<ProjectFileRename>,
    #[doc(hidden)]
    pub(crate) patches: Vec<ProjectFilePatch>,
    #[doc(hidden)]
    pub(crate) entrypoint: Option<String>,
    #[doc(hidden)]
    pub(crate) requirements: Option<Vec<DirectRequirement>>,
    pub(crate) locks: Option<Vec<LibraryLock>>,
    pub(crate) dependency_guidance: Vec<DependencyGuidance>,
    #[doc(hidden)]
    pub(crate) hint_patches: Vec<ExactPatch>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", deny_unknown_fields)]
pub enum ProjectOperation {
    #[serde(rename = "file.add")]
    FileAdd { path: String, content: String },
    #[serde(rename = "file.patch")]
    FilePatch {
        path: String,
        patches: Vec<ExactPatch>,
    },
    #[serde(rename = "file.delete")]
    FileDelete { path: String },
    #[serde(rename = "file.rename")]
    FileRename { from: String, to: String },
    #[serde(rename = "entrypoint.set")]
    EntrypointSet { path: String },
    #[serde(rename = "dependencies.set")]
    DependenciesSet {
        requirements: Vec<DirectRequirement>,
    },
    #[serde(rename = "hints.patch")]
    HintsPatch { patches: Vec<ExactPatch> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectFileRename {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectFilePatch {
    pub path: String,
    pub patches: Vec<ExactPatch>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactPatch {
    pub old: String,
    pub new: String,
}

impl ProjectBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        files: Vec<ProjectFile>,
        entrypoint: String,
        requirements: Vec<DirectRequirement>,
        locks: Vec<LibraryLock>,
        dependency_guidance: &[DependencyGuidance],
        hints: &str,
    ) -> Result<Self, RepositoryError> {
        let mut caller_files = normalize_caller_files(files)?;
        let entrypoint = normalize_user_path(&entrypoint)?;
        normalize_dependencies(
            &mut caller_files,
            &entrypoint,
            requirements,
            locks,
            dependency_guidance,
            hints,
        )
    }

    pub fn single_source(source: &[u8]) -> Result<Self, RepositoryError> {
        let content = std::str::from_utf8(source).map_err(|_| RepositoryError::Invalid)?;
        Self::new(
            vec![ProjectFile {
                path: "source.py".to_owned(),
                content: content.to_owned(),
            }],
            "source.py".to_owned(),
            Vec::new(),
            Vec::new(),
            &[],
            "",
        )
    }

    pub fn apply(&self, edit: &ProjectEdit) -> Result<Self, RepositoryError> {
        let operations = edit.ordered_operations()?;
        let mut files = self.caller_files();
        let mut entrypoint = self.entrypoint.clone();
        let mut requirements = self.requirements.clone();
        let mut locks = self.locks.clone();
        let mut hints = self.hints()?.to_owned();

        for operation in operations {
            match operation {
                ProjectOperation::FileAdd { path, content } => {
                    let path = normalize_user_path(&path)?;
                    if files.iter().any(|file| file.path == path) {
                        return Err(RepositoryError::Invalid);
                    }
                    files.push(ProjectFile {
                        path,
                        content: normalize_text(&content),
                    });
                }
                ProjectOperation::FilePatch { path, patches } => {
                    let path = normalize_user_path(&path)?;
                    let file = files
                        .iter_mut()
                        .find(|file| file.path == path)
                        .ok_or(RepositoryError::Invalid)?;
                    apply_exact_patches(&mut file.content, &patches)?;
                }
                ProjectOperation::FileDelete { path } => {
                    let path = normalize_user_path(&path)?;
                    if path == entrypoint {
                        return Err(RepositoryError::Invalid);
                    }
                    let position = files
                        .iter()
                        .position(|file| file.path == path)
                        .ok_or(RepositoryError::Invalid)?;
                    files.remove(position);
                }
                ProjectOperation::FileRename { from, to } => {
                    let from = normalize_user_path(&from)?;
                    let to = normalize_user_path(&to)?;
                    if files.iter().any(|file| file.path == to) {
                        return Err(RepositoryError::Invalid);
                    }
                    let file = files
                        .iter_mut()
                        .find(|file| file.path == from)
                        .ok_or(RepositoryError::Invalid)?;
                    file.path.clone_from(&to);
                    if entrypoint == from {
                        entrypoint = to;
                    }
                }
                ProjectOperation::EntrypointSet { path } => {
                    entrypoint = normalize_user_path(&path)?;
                }
                ProjectOperation::DependenciesSet {
                    requirements: value,
                } => requirements = value,
                ProjectOperation::HintsPatch { patches } => {
                    apply_exact_patches(&mut hints, &patches)?;
                }
            }
        }
        if let Some(value) = &edit.locks {
            locks.clone_from(value);
        }
        let guidance: &[DependencyGuidance] = if requirements.is_empty() {
            &[]
        } else {
            &edit.dependency_guidance
        };
        let next = Self::new(files, entrypoint, requirements, locks, guidance, &hints)?;
        if &next == self {
            return Err(RepositoryError::Invalid);
        }
        Ok(next)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RepositoryError> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| RepositoryError::Corrupt)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_CANONICAL_BUNDLE_BYTES {
            return Err(RepositoryError::Invalid);
        }
        Ok(bytes)
    }

    pub fn digest(&self) -> Result<String, RepositoryError> {
        Ok(hex_digest(Sha256::digest(self.canonical_bytes()?)))
    }

    pub fn agents_md(&self) -> Result<&str, RepositoryError> {
        self.files
            .iter()
            .find(|file| file.path == AGENTS_PATH)
            .map(|file| file.content.as_str())
            .ok_or(RepositoryError::Corrupt)
    }

    pub fn hints(&self) -> Result<&str, RepositoryError> {
        self.agents_md()?
            .split_once("# Hints\n\n")
            .map(|(_, hints)| hints.strip_suffix('\n').unwrap_or(hints))
            .ok_or(RepositoryError::Corrupt)
    }

    #[must_use]
    pub fn caller_files(&self) -> Vec<ProjectFile> {
        self.files
            .iter()
            .filter(|file| file.path != AGENTS_PATH)
            .cloned()
            .collect()
    }
}

impl ProjectEdit {
    pub fn new(operations: Vec<ProjectOperation>) -> Result<Self, RepositoryError> {
        let edit = Self {
            operations,
            ..Self::default()
        };
        edit.ordered_operations()?;
        Ok(edit)
    }

    pub(crate) fn lock_update(
        locks: Vec<LibraryLock>,
        dependency_guidance: Vec<DependencyGuidance>,
    ) -> Self {
        Self {
            locks: Some(locks),
            dependency_guidance,
            ..Self::default()
        }
    }

    fn ordered_operations(&self) -> Result<Vec<ProjectOperation>, RepositoryError> {
        let legacy_count = self.add_files.len()
            + self.delete_files.len()
            + self.renames.len()
            + self.patches.len()
            + usize::from(self.entrypoint.is_some())
            + usize::from(self.requirements.is_some())
            + usize::from(!self.hint_patches.is_empty());
        if !self.operations.is_empty() && legacy_count != 0 {
            return Err(RepositoryError::Invalid);
        }
        let operations = if self.operations.is_empty() {
            let mut operations = Vec::with_capacity(legacy_count);
            operations.extend(self.add_files.iter().map(|file| ProjectOperation::FileAdd {
                path: file.path.clone(),
                content: file.content.clone(),
            }));
            operations.extend(
                self.delete_files
                    .iter()
                    .map(|path| ProjectOperation::FileDelete { path: path.clone() }),
            );
            operations.extend(
                self.renames
                    .iter()
                    .map(|rename| ProjectOperation::FileRename {
                        from: rename.from.clone(),
                        to: rename.to.clone(),
                    }),
            );
            operations.extend(
                self.patches
                    .iter()
                    .map(|patch| ProjectOperation::FilePatch {
                        path: patch.path.clone(),
                        patches: patch.patches.clone(),
                    }),
            );
            if let Some(path) = &self.entrypoint {
                operations.push(ProjectOperation::EntrypointSet { path: path.clone() });
            }
            if let Some(requirements) = &self.requirements {
                operations.push(ProjectOperation::DependenciesSet {
                    requirements: requirements.clone(),
                });
            }
            if !self.hint_patches.is_empty() {
                operations.push(ProjectOperation::HintsPatch {
                    patches: self.hint_patches.clone(),
                });
            }
            operations
        } else {
            self.operations.clone()
        };
        let internal_only =
            operations.is_empty() && self.locks.is_some() && !self.dependency_guidance.is_empty();
        if (!internal_only && operations.is_empty()) || operations.len() > 256 {
            return Err(RepositoryError::Invalid);
        }
        if operations.iter().any(|operation| match operation {
            ProjectOperation::FilePatch { patches, .. }
            | ProjectOperation::HintsPatch { patches } => patches.is_empty() || patches.len() > 256,
            _ => false,
        }) {
            return Err(RepositoryError::Invalid);
        }
        Ok(operations)
    }

    fn final_requirements(&self) -> Result<Option<Vec<DirectRequirement>>, RepositoryError> {
        Ok(self
            .ordered_operations()?
            .into_iter()
            .filter_map(|operation| match operation {
                ProjectOperation::DependenciesSet { requirements } => Some(requirements),
                _ => None,
            })
            .next_back())
    }
}

#[allow(clippy::too_many_arguments)]
fn normalize_dependencies(
    caller_files: &mut Vec<ProjectFile>,
    entrypoint: &str,
    mut requirements: Vec<DirectRequirement>,
    mut locks: Vec<LibraryLock>,
    dependency_guidance: &[DependencyGuidance],
    hints: &str,
) -> Result<ProjectBundle, RepositoryError> {
    if caller_files.is_empty() || caller_files.len() > MAX_PROJECT_FILES {
        return Err(RepositoryError::Invalid);
    }
    let entrypoint_file = caller_files
        .iter()
        .find(|file| file.path == entrypoint)
        .ok_or(RepositoryError::Invalid)?;
    if std::path::Path::new(entrypoint).extension() != Some(std::ffi::OsStr::new("py"))
        || entrypoint_file.content.is_empty()
    {
        return Err(RepositoryError::Invalid);
    }
    requirements.sort_by(|left, right| left.name.cmp(&right.name));
    locks.sort_by(|left, right| left.name.cmp(&right.name));
    validate_dependencies(&requirements, &locks)?;
    let hints = normalize_hints(hints)?;
    let agents = render_agents(
        caller_files,
        entrypoint,
        &locks,
        dependency_guidance,
        &hints,
    )?;
    caller_files.push(ProjectFile {
        path: AGENTS_PATH.to_owned(),
        content: agents,
    });
    caller_files.sort_by(|left, right| left.path.cmp(&right.path));
    let bundle = ProjectBundle {
        format: FORMAT.to_owned(),
        entrypoint: entrypoint.to_owned(),
        requirements,
        locks,
        files: caller_files.clone(),
    };
    bundle.canonical_bytes()?;
    Ok(bundle)
}

fn normalize_caller_files(files: Vec<ProjectFile>) -> Result<Vec<ProjectFile>, RepositoryError> {
    let mut normalized = Vec::with_capacity(files.len());
    let mut paths = BTreeSet::new();
    let mut total = 0usize;
    for file in files {
        let path = normalize_user_path(&file.path)?;
        if !paths.insert(path.clone()) {
            return Err(RepositoryError::Invalid);
        }
        let content = normalize_text(&file.content);
        if content.len() > MAX_PROJECT_FILE_BYTES {
            return Err(RepositoryError::Invalid);
        }
        total = total
            .checked_add(content.len())
            .ok_or(RepositoryError::Invalid)?;
        normalized.push(ProjectFile { path, content });
    }
    if total > MAX_CALLER_CONTENT_BYTES {
        return Err(RepositoryError::Invalid);
    }
    normalized.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(normalized)
}

fn validate_dependencies(
    requirements: &[DirectRequirement],
    locks: &[LibraryLock],
) -> Result<(), RepositoryError> {
    if requirements.len() > MAX_PROJECT_REQUIREMENTS || requirements.len() != locks.len() {
        return Err(RepositoryError::Invalid);
    }
    let mut names = BTreeSet::new();
    for (requirement, lock) in requirements.iter().zip(locks) {
        validate_library_name(&requirement.name)?;
        let (major, parsed) = parse_requirement(&requirement.range)?;
        if !names.insert(requirement.name.as_str())
            || lock.name != requirement.name
            || lock.version.major != major
            || !lock.version.pre.is_empty()
            || !lock.version.build.is_empty()
            || !parsed.matches(&lock.version)
        {
            return Err(RepositoryError::Invalid);
        }
        validate_revision(&lock.release_sha256)?;
    }
    Ok(())
}

pub fn parse_requirement(value: &str) -> Result<(u64, VersionReq), RepositoryError> {
    let requirement = VersionReq::parse(value).map_err(|_| RepositoryError::Invalid)?;
    let [lower, upper] = requirement.comparators.as_slice() else {
        return Err(RepositoryError::Invalid);
    };
    if lower.op != Op::GreaterEq
        || lower.minor.is_none()
        || lower.patch.is_none()
        || !lower.pre.is_empty()
        || upper.op != Op::Less
        || upper.minor != Some(0)
        || upper.patch != Some(0)
        || !upper.pre.is_empty()
        || upper.major != lower.major.checked_add(1).ok_or(RepositoryError::Invalid)?
    {
        return Err(RepositoryError::Invalid);
    }
    let canonical = format!(
        ">={}.{}.{},<{}.0.0",
        lower.major,
        lower.minor.unwrap_or(0),
        lower.patch.unwrap_or(0),
        upper.major
    );
    if value != canonical {
        return Err(RepositoryError::Invalid);
    }
    Ok((lower.major, requirement))
}

pub fn validate_library_name(name: &str) -> Result<(), RepositoryError> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(RepositoryError::Invalid);
    }
    Ok(())
}

pub(crate) fn normalize_user_path(path: &str) -> Result<String, RepositoryError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.len() > MAX_PROJECT_PATH_BYTES
        || path == AGENTS_PATH
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path
            .chars()
            .any(|character| character == '\0' || character.is_ascii_control())
        || path.split('/').any(|component| {
            component.is_empty()
                || component == "."
                || component == ".."
                || component.len() > MAX_PROJECT_COMPONENT_BYTES
        })
    {
        return Err(RepositoryError::Invalid);
    }
    Ok(path.to_owned())
}

pub(crate) fn normalize_text(value: &str) -> String {
    value
        .strip_prefix('\u{feff}')
        .unwrap_or(value)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn normalize_hints(value: &str) -> Result<String, RepositoryError> {
    let normalized = normalize_text(value).trim_end_matches('\n').to_owned();
    if normalized
        .lines()
        .any(|line| markdown_heading_level(line) == Some(1))
    {
        return Err(RepositoryError::Invalid);
    }
    Ok(normalized)
}

fn render_agents(
    files: &[ProjectFile],
    entrypoint: &str,
    locks: &[LibraryLock],
    guidance: &[DependencyGuidance],
    hints: &str,
) -> Result<String, RepositoryError> {
    if locks.len() != guidance.len() {
        return Err(RepositoryError::Invalid);
    }
    let mut output = format!("# Index\n\nEntrypoint: {}\n", json_quote(entrypoint)?);
    let mut paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    paths.push(AGENTS_PATH);
    paths.sort_unstable();
    output.push('\n');
    for path in paths {
        writeln!(output, "- {}", json_quote(path)?).map_err(|_| RepositoryError::Corrupt)?;
    }
    output.push_str("\n# Dependency Guidance\n\n");
    if locks.is_empty() {
        output.push_str("No shared libraries are locked.\n");
    } else {
        for lock in locks {
            let item = guidance
                .iter()
                .find(|item| {
                    item.name == lock.name
                        && item.version == lock.version
                        && item.release_sha256 == lock.release_sha256
                })
                .ok_or(RepositoryError::Invalid)?;
            let guidance_text = normalize_text(&item.guidance)
                .trim_end_matches('\n')
                .to_owned();
            if guidance_text.is_empty()
                || guidance_text
                    .lines()
                    .any(|line| markdown_heading_level(line).is_some_and(|level| level <= 2))
            {
                return Err(RepositoryError::Invalid);
            }
            writeln!(output, "## faktory_shared.{} {}\n\nRelease: {}\n\n{}\n\nDocumentation through MCP library.get:\n", lock.name, lock.version, lock.release_sha256, guidance_text).map_err(|_| RepositoryError::Corrupt)?;
            let mut docs = item.documentation.clone();
            docs.sort();
            for doc in docs {
                writeln!(output, "- {}", json_quote(&doc)?)
                    .map_err(|_| RepositoryError::Corrupt)?;
            }
            output.push('\n');
        }
        output.pop();
    }
    output.push_str("\n# Hints\n\n");
    if !hints.is_empty() {
        output.push_str(hints);
        output.push('\n');
    }
    if output.len() > MAX_PROJECT_FILE_BYTES {
        return Err(RepositoryError::Invalid);
    }
    Ok(output)
}

fn json_quote(value: &str) -> Result<String, RepositoryError> {
    serde_json::to_string(value).map_err(|_| RepositoryError::Corrupt)
}

pub(crate) fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    (level > 0
        && level <= 6
        && trimmed
            .as_bytes()
            .get(level)
            .is_none_or(u8::is_ascii_whitespace))
    .then_some(level)
}

fn apply_exact_patches(value: &mut String, patches: &[ExactPatch]) -> Result<(), RepositoryError> {
    if patches.is_empty() || patches.len() > 256 {
        return Err(RepositoryError::Invalid);
    }
    for patch in patches {
        if patch.old.is_empty()
            || value
                .as_bytes()
                .windows(patch.old.len())
                .filter(|candidate| *candidate == patch.old.as_bytes())
                .take(2)
                .count()
                != 1
        {
            return Err(RepositoryError::Invalid);
        }
        *value = value.replacen(&patch.old, &patch.new, 1);
    }
    Ok(())
}

pub(crate) fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("write String");
            output
        })
}

#[must_use]
pub fn project_key(model_id: &str, revision: &str) -> String {
    format!("models/{model_id}/revisions/{revision}/project.json")
}

impl Repository {
    pub async fn resolve_project_requirements(
        &self,
        requirements: &[DirectRequirement],
    ) -> Result<(Vec<LibraryLock>, Vec<DependencyGuidance>), RepositoryError> {
        let mut ordered = requirements.to_vec();
        ordered.sort_by(|left, right| left.name.cmp(&right.name));
        if ordered.len() > MAX_PROJECT_REQUIREMENTS
            || ordered.windows(2).any(|pair| pair[0].name == pair[1].name)
        {
            return Err(RepositoryError::Invalid);
        }
        let mut locks = Vec::with_capacity(ordered.len());
        for requirement in ordered {
            validate_library_name(&requirement.name)?;
            parse_requirement(&requirement.range)?;
            let release = self
                .resolve_library(&requirement.name, &requirement.range)
                .await?;
            locks.push(LibraryLock {
                name: release.name,
                version: release.version,
                release_sha256: release.digest,
            });
        }
        let guidance = self.dependency_guidance(&locks).await?;
        Ok((locks, guidance))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_project_from_files(
        &self,
        model_id: &str,
        name: &str,
        files: Vec<ProjectFile>,
        entrypoint: String,
        requirements: Vec<DirectRequirement>,
        hints: &str,
    ) -> Result<ModelRecord, RepositoryError> {
        let (locks, guidance) = self.resolve_project_requirements(&requirements).await?;
        let project = ProjectBundle::new(files, entrypoint, requirements, locks, &guidance, hints)?;
        self.create_project(model_id, name, project).await
    }

    pub async fn create_project(
        &self,
        model_id: &str,
        name: &str,
        project: ProjectBundle,
    ) -> Result<ModelRecord, RepositoryError> {
        validate_model_id(model_id)?;
        validate_name(name)?;
        self.validate_project(&project).await?;
        let (resolved, _) = self
            .resolve_project_requirements(&project.requirements)
            .await?;
        if project.locks != resolved {
            return Err(RepositoryError::Invalid);
        }
        let _guard = self.mutations.lock().await;
        match self.get_model(model_id).await {
            Ok(_) => return Err(RepositoryError::Conflict),
            Err(RepositoryError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let revision = project.digest()?;
        self.put_immutable(
            &project_key(model_id, &revision),
            Bytes::from(project.canonical_bytes()?),
        )
        .await?;
        let entrypoint = project
            .files
            .iter()
            .find(|file| file.path == project.entrypoint)
            .ok_or(RepositoryError::Corrupt)?;
        self.put_immutable(
            &super::source_key(model_id, &revision),
            Bytes::copy_from_slice(entrypoint.content.as_bytes()),
        )
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
            current_successful_outputs: Vec::new(),
            updated_at: mutation_timestamp(None)?,
        };
        self.save_model(&record, PutCondition::Absent).await?;
        Ok(record)
    }

    pub async fn get_project(
        &self,
        model_id: &str,
        revision: &str,
    ) -> Result<ProjectBundle, RepositoryError> {
        validate_model_id(model_id)?;
        validate_revision(revision)?;
        let object = self.store.get(&project_key(model_id, revision)).await?;
        if !object.bytes.ends_with(b"\n") {
            return Err(RepositoryError::Corrupt);
        }
        let stored: ProjectBundle =
            serde_json::from_slice(&object.bytes).map_err(|_| RepositoryError::Corrupt)?;
        self.validate_project(&stored)
            .await
            .map_err(|_| RepositoryError::Corrupt)?;
        if stored
            .canonical_bytes()
            .map_err(|_| RepositoryError::Corrupt)?
            != object.bytes
            || stored.digest().map_err(|_| RepositoryError::Corrupt)? != revision
        {
            return Err(RepositoryError::Corrupt);
        }
        Ok(stored)
    }

    async fn validate_project(&self, project: &ProjectBundle) -> Result<(), RepositoryError> {
        if project.format != FORMAT {
            return Err(RepositoryError::Invalid);
        }
        let hints = project.hints()?.to_owned();
        let guidance = self.dependency_guidance(&project.locks).await?;
        let canonical = ProjectBundle::new(
            project.caller_files(),
            project.entrypoint.clone(),
            project.requirements.clone(),
            project.locks.clone(),
            &guidance,
            &hints,
        )?;
        if &canonical != project {
            return Err(RepositoryError::Invalid);
        }
        Ok(())
    }

    pub(super) async fn dependency_guidance(
        &self,
        locks: &[LibraryLock],
    ) -> Result<Vec<DependencyGuidance>, RepositoryError> {
        let mut guidance = Vec::with_capacity(locks.len());
        for lock in locks {
            let release = self.get_library(&lock.name, &lock.version).await?;
            if release.digest != lock.release_sha256 {
                return Err(RepositoryError::Corrupt);
            }
            guidance.push(DependencyGuidance {
                name: lock.name.clone(),
                version: lock.version.clone(),
                release_sha256: lock.release_sha256.clone(),
                guidance: release.guidance.clone(),
                documentation: release.docs.iter().map(|doc| doc.path.clone()).collect(),
            });
        }
        Ok(guidance)
    }

    pub async fn edit_project(
        &self,
        model_id: &str,
        expected_revision: &str,
        name: Option<&str>,
        edit: Option<&ProjectEdit>,
    ) -> Result<EditedModel, RepositoryError> {
        validate_model_id(model_id)?;
        validate_revision(expected_revision)?;
        if name.is_none() && edit.is_none() {
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
        let source_changed = if let Some(edit) = edit {
            let project = self.get_project(model_id, expected_revision).await?;
            let mut resolved_edit = edit.clone();
            if let Some(requirements) = resolved_edit.final_requirements()? {
                let (locks, guidance) = self.resolve_project_requirements(&requirements).await?;
                resolved_edit.locks = Some(locks);
                resolved_edit.dependency_guidance = guidance;
            } else if !project.requirements.is_empty()
                && resolved_edit.dependency_guidance.is_empty()
            {
                let locks = resolved_edit.locks.as_ref().unwrap_or(&project.locks);
                resolved_edit.dependency_guidance = self.dependency_guidance(locks).await?;
            }
            let next = project.apply(&resolved_edit)?;
            let revision = next.digest()?;
            self.put_immutable(
                &project_key(model_id, &revision),
                Bytes::from(next.canonical_bytes()?),
            )
            .await?;
            let entrypoint = next
                .files
                .iter()
                .find(|file| file.path == next.entrypoint)
                .ok_or(RepositoryError::Corrupt)?;
            self.put_immutable(
                &super::source_key(model_id, &revision),
                Bytes::copy_from_slice(entrypoint.content.as_bytes()),
            )
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            content: content.to_owned(),
        }
    }
    fn bundle(files: Vec<ProjectFile>) -> ProjectBundle {
        ProjectBundle::new(
            files,
            "main.py".to_owned(),
            Vec::new(),
            Vec::new(),
            &[],
            "Edit me",
        )
        .expect("bundle")
    }

    #[test]
    fn canonical_hash_is_order_independent_and_exact_json() {
        let left = bundle(vec![file("main.py", "a\r\n"), file("lib/util.py", "b")]);
        let right = bundle(vec![file("lib/util.py", "b"), file("main.py", "a\n")]);
        assert_eq!(left, right);
        assert_eq!(left.digest(), right.digest());
        assert_ne!(
            left.digest().expect("digest"),
            hex_digest(Sha256::digest(b"a\n"))
        );
        let bytes = left.canonical_bytes().expect("canonical");
        assert!(bytes.starts_with(b"{\"format\":\"faktory-project-v1\",\"entrypoint\":"));
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn empty_agents_document_has_the_exact_managed_shape() {
        let project = ProjectBundle::new(
            vec![file("main.py", "part = 1")],
            "main.py".to_owned(),
            Vec::new(),
            Vec::new(),
            &[],
            "",
        )
        .expect("project");
        assert_eq!(
            project.agents_md().expect("agents"),
            "# Index\n\nEntrypoint: \"main.py\"\n\n- \"AGENTS.md\"\n- \"main.py\"\n\n# Dependency Guidance\n\nNo shared libraries are locked.\n\n# Hints\n\n"
        );
    }

    #[test]
    fn validation_rejects_unsafe_and_non_ascii_paths() {
        for path in [
            "",
            "/main.py",
            "./main.py",
            "a/../main.py",
            "a\\main.py",
            AGENTS_PATH,
        ] {
            assert_eq!(
                ProjectBundle::new(
                    vec![file(path, "x")],
                    path.to_owned(),
                    Vec::new(),
                    Vec::new(),
                    &[],
                    ""
                ),
                Err(RepositoryError::Invalid)
            );
        }
        for path in ["\u{e9}.py", "e\u{301}.py"] {
            assert_eq!(
                ProjectBundle::new(
                    vec![file(path, "x")],
                    path.to_owned(),
                    Vec::new(),
                    Vec::new(),
                    &[],
                    ""
                ),
                Err(RepositoryError::Invalid)
            );
        }
        assert_eq!(
            ProjectBundle::single_source(&[0xff]),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            ProjectBundle::new(
                vec![file("main.py", "")],
                "main.py".to_owned(),
                Vec::new(),
                Vec::new(),
                &[],
                ""
            ),
            Err(RepositoryError::Invalid)
        );
    }

    #[test]
    fn protected_agents_content_can_only_be_regenerated() {
        let project = bundle(vec![file("main.py", "old")]);
        assert_eq!(
            project.apply(&ProjectEdit {
                operations: vec![ProjectOperation::FilePatch {
                    path: AGENTS_PATH.to_owned(),
                    patches: vec![ExactPatch {
                        old: "Index".to_owned(),
                        new: "Owned".to_owned()
                    }]
                }],
                ..ProjectEdit::default()
            }),
            Err(RepositoryError::Invalid)
        );
        let changed = project
            .apply(&ProjectEdit {
                operations: vec![ProjectOperation::HintsPatch {
                    patches: vec![ExactPatch {
                        old: "Edit me".to_owned(),
                        new: "New hints".to_owned(),
                    }],
                }],
                ..ProjectEdit::default()
            })
            .expect("hints edit");
        assert!(
            changed
                .agents_md()
                .expect("agents")
                .starts_with("# Index\n")
        );
        assert!(
            changed
                .agents_md()
                .expect("agents")
                .ends_with("# Hints\n\nNew hints\n")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn ordered_operations_are_sequential_transactional_and_bounded() {
        let project = bundle(vec![file("main.py", "old")]);
        assert_eq!(ProjectEdit::new(Vec::new()), Err(RepositoryError::Invalid));
        assert!(
            ProjectEdit::new(vec![
                ProjectOperation::EntrypointSet {
                    path: "main.py".to_owned(),
                };
                256
            ])
            .is_ok()
        );
        for patches in [
            Vec::new(),
            vec![
                ExactPatch {
                    old: "old".to_owned(),
                    new: "new".to_owned(),
                };
                257
            ],
        ] {
            assert_eq!(
                ProjectEdit::new(vec![ProjectOperation::FilePatch {
                    path: "main.py".to_owned(),
                    patches,
                }]),
                Err(RepositoryError::Invalid)
            );
        }
        let changed = project
            .apply(&ProjectEdit {
                operations: vec![
                    ProjectOperation::FileAdd {
                        path: "next.py".to_owned(),
                        content: "temporary".to_owned(),
                    },
                    ProjectOperation::FilePatch {
                        path: "next.py".to_owned(),
                        patches: vec![ExactPatch {
                            old: "temporary".to_owned(),
                            new: "final".to_owned(),
                        }],
                    },
                    ProjectOperation::FileRename {
                        from: "main.py".to_owned(),
                        to: "renamed.py".to_owned(),
                    },
                    ProjectOperation::FilePatch {
                        path: "renamed.py".to_owned(),
                        patches: vec![ExactPatch {
                            old: "old".to_owned(),
                            new: "updated".to_owned(),
                        }],
                    },
                    ProjectOperation::EntrypointSet {
                        path: "next.py".to_owned(),
                    },
                ],
                ..ProjectEdit::default()
            })
            .expect("ordered edit");
        assert_eq!(changed.entrypoint, "next.py");
        assert_eq!(
            changed
                .files
                .iter()
                .find(|file| file.path == "next.py")
                .expect("added file")
                .content,
            "final"
        );
        assert_eq!(
            changed
                .files
                .iter()
                .find(|file| file.path == "renamed.py")
                .expect("renamed file")
                .content,
            "updated"
        );

        assert_eq!(
            project.apply(&ProjectEdit {
                operations: vec![
                    ProjectOperation::FileAdd {
                        path: "temporary.py".to_owned(),
                        content: String::new(),
                    },
                    ProjectOperation::FileDelete {
                        path: "temporary.py".to_owned(),
                    },
                ],
                ..ProjectEdit::default()
            }),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            project.apply(&ProjectEdit {
                operations: vec![
                    ProjectOperation::EntrypointSet {
                        path: "main.py".to_owned(),
                    };
                    257
                ],
                ..ProjectEdit::default()
            }),
            Err(RepositoryError::Invalid)
        );
    }

    #[test]
    fn only_exact_same_major_ranges_are_accepted() {
        assert!(parse_requirement(">=1.2.3,<2.0.0").is_ok());
        for invalid in [">=1.2,<2", ">=1.2.3, <2.0.0", "^1.2.3", ">=1.2.3,<3.0.0"] {
            assert_eq!(parse_requirement(invalid), Err(RepositoryError::Invalid));
        }
    }
}
