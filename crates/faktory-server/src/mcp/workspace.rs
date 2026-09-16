//! Bounded agent-oriented project and library workspace operations.

use globset::GlobBuilder;
use regex::Regex;
use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::model::{
    ModelRecord, Repository, RepositoryError,
    project::{
        AGENTS_PATH, ExactPatch, ProjectBundle, ProjectOperation, hex_digest, normalize_user_path,
    },
};

use super::{LibraryReadInput, ModelGrepInput, ModelReadInput};

pub(super) const DEFAULT_READ_LIMIT: usize = 200;
pub(super) const MAX_READ_LIMIT: usize = 2_000;
pub(super) const MAX_GLOB_RESULTS: usize = 1_000;
pub(super) const DEFAULT_GREP_LIMIT: usize = 100;
pub(super) const MAX_GREP_LIMIT: usize = 1_000;
pub(super) const MAX_PATTERN_BYTES: usize = 1_024;
pub(super) const MAX_MATCH_TEXT_BYTES: usize = 2_000;
pub(super) const MAX_PATCH_BYTES: usize = 1_048_576;

#[derive(Debug)]
pub(super) struct ParsedPatch {
    pub operations: Vec<ProjectOperation>,
    pub changed_paths: Vec<String>,
}

pub(super) async fn model_open(
    repository: &Repository,
    model_id: &str,
) -> Result<Value, RepositoryError> {
    let (model, project, revision) = load_project(repository, model_id, None).await?;
    let agents_md = project.agents_md()?;
    Ok(json!({
        "model": model,
        "revision": revision,
        "entrypoint": project.entrypoint,
        "requirements": project.requirements,
        "locks": project.locks,
        "files": file_index(&project.files),
        "agents_md": agents_md
    }))
}

pub(super) async fn model_read(
    repository: &Repository,
    input: &ModelReadInput,
) -> Result<Value, RepositoryError> {
    validate_read_bounds(input.offset, input.limit)?;
    let (_, project, actual_revision) =
        load_project(repository, &input.model_id, input.revision.as_deref()).await?;
    let path = validate_project_read_path(&input.path)?;
    let file = project
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or(RepositoryError::NotFound)?;
    let read = read_lines(&file.content, input.offset, input.limit)?;
    Ok(json!({
        "revision": actual_revision,
        "path": path,
        "content": read.content,
        "start_line": input.offset,
        "total_lines": read.total_lines,
        "truncated": read.truncated
    }))
}

pub(super) async fn model_glob(
    repository: &Repository,
    model_id: &str,
    pattern: &str,
    revision: Option<&str>,
) -> Result<Value, RepositoryError> {
    let matcher = compile_glob(pattern)?;
    let (_, project, actual_revision) = load_project(repository, model_id, revision).await?;
    let mut paths = project
        .files
        .iter()
        .filter(|file| matcher.is_match(&file.path))
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let truncated = paths.len() > MAX_GLOB_RESULTS;
    paths.truncate(MAX_GLOB_RESULTS);
    Ok(json!({"revision": actual_revision, "paths": paths, "truncated": truncated}))
}

pub(super) async fn model_grep(
    repository: &Repository,
    input: &ModelGrepInput,
) -> Result<Value, RepositoryError> {
    if input.pattern.is_empty()
        || input.pattern.len() > MAX_PATTERN_BYTES
        || input.limit == 0
        || input.limit > MAX_GREP_LIMIT
    {
        return Err(RepositoryError::Invalid);
    }
    let regex = Regex::new(&input.pattern).map_err(|_| RepositoryError::Invalid)?;
    let include = input.include.as_deref().map(compile_glob).transpose()?;
    let (_, project, actual_revision) =
        load_project(repository, &input.model_id, input.revision.as_deref()).await?;
    let mut matches = Vec::new();
    let mut truncated = false;
    'files: for file in &project.files {
        if include
            .as_ref()
            .is_some_and(|matcher| !matcher.is_match(&file.path))
        {
            continue;
        }
        for (index, line) in logical_lines(&file.content).enumerate() {
            let line = line.strip_suffix('\n').unwrap_or(line);
            if regex.is_match(line) {
                if matches.len() == input.limit {
                    truncated = true;
                    break 'files;
                }
                let (text, text_truncated) = truncate_utf8(line, MAX_MATCH_TEXT_BYTES);
                matches.push(json!({
                    "path": file.path,
                    "line": index + 1,
                    "text": text,
                    "text_truncated": text_truncated
                }));
            }
        }
    }
    Ok(json!({"revision": actual_revision, "matches": matches, "truncated": truncated}))
}

pub(super) async fn library_open(
    repository: &Repository,
    name: &str,
    version: &Version,
) -> Result<Value, RepositoryError> {
    let release = repository.get_library(name, version).await?;
    Ok(json!({
        "name": release.name,
        "version": release.version,
        "release_sha256": release.digest,
        "guidance": release.guidance,
        "docs": file_index(&release.docs),
        "files": file_index(&release.files)
    }))
}

pub(super) async fn library_read(
    repository: &Repository,
    input: &LibraryReadInput,
) -> Result<Value, RepositoryError> {
    validate_read_bounds(input.offset, input.limit)?;
    let release = repository.get_library(&input.name, &input.version).await?;
    let path = normalize_user_path(&input.path)?;
    let file = release
        .docs
        .iter()
        .chain(&release.files)
        .find(|file| file.path == path)
        .ok_or(RepositoryError::NotFound)?;
    let read = read_lines(&file.content, input.offset, input.limit)?;
    Ok(json!({
        "name": release.name,
        "version": release.version,
        "release_sha256": release.digest,
        "path": path,
        "content": read.content,
        "start_line": input.offset,
        "total_lines": read.total_lines,
        "truncated": read.truncated
    }))
}

pub(super) fn parse_patch(patch: &str) -> Result<ParsedPatch, RepositoryError> {
    if patch.is_empty() || patch.len() > MAX_PATCH_BYTES || patch.contains(['\r', '\0']) {
        return Err(RepositoryError::Invalid);
    }
    let patch = patch.strip_suffix('\n').unwrap_or(patch);
    let lines = patch.split('\n').collect::<Vec<_>>();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(RepositoryError::Invalid);
    }
    let mut parser = PatchParser {
        lines: &lines,
        index: 1,
        operations: Vec::new(),
        changed_paths: Vec::new(),
    };
    while parser.index + 1 < parser.lines.len() {
        let line = parser.lines[parser.index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            parser.parse_add(path)?;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            parser.parse_update(path)?;
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            parser.parse_delete(path)?;
        } else {
            return Err(RepositoryError::Invalid);
        }
    }
    if parser.index + 1 != parser.lines.len() || parser.operations.is_empty() {
        return Err(RepositoryError::Invalid);
    }
    Ok(ParsedPatch {
        operations: parser.operations,
        changed_paths: parser.changed_paths,
    })
}

struct PatchParser<'a> {
    lines: &'a [&'a str],
    index: usize,
    operations: Vec<ProjectOperation>,
    changed_paths: Vec<String>,
}

impl PatchParser<'_> {
    fn parse_add(&mut self, path: &str) -> Result<(), RepositoryError> {
        let path = validate_patch_path(path)?;
        self.index += 1;
        let mut content = Vec::new();
        while !self.at_section_boundary() {
            let line = self.lines[self.index]
                .strip_prefix('+')
                .ok_or(RepositoryError::Invalid)?;
            content.push(line);
            self.index += 1;
        }
        self.changed_paths.push(path.clone());
        self.operations.push(ProjectOperation::FileAdd {
            path,
            content: content.join("\n"),
        });
        Ok(())
    }

    fn parse_delete(&mut self, path: &str) -> Result<(), RepositoryError> {
        let path = validate_patch_path(path)?;
        self.index += 1;
        if !self.at_section_boundary() {
            return Err(RepositoryError::Invalid);
        }
        self.changed_paths.push(path.clone());
        self.operations.push(ProjectOperation::FileDelete { path });
        Ok(())
    }

    fn parse_update(&mut self, path: &str) -> Result<(), RepositoryError> {
        let path = validate_patch_path(path)?;
        self.index += 1;
        let mut patches = Vec::new();
        while self.index + 1 < self.lines.len() && self.lines[self.index].starts_with("@@") {
            let heading = self.lines[self.index];
            if !valid_hunk_header(heading) {
                return Err(RepositoryError::Invalid);
            }
            self.index += 1;
            patches.push(self.parse_hunk()?);
        }
        self.changed_paths.push(path.clone());
        let has_patches = !patches.is_empty();
        if has_patches {
            self.operations.push(ProjectOperation::FilePatch {
                path: path.clone(),
                patches,
            });
        }
        let moved = if self.index + 1 < self.lines.len()
            && let Some(to) = self.lines[self.index].strip_prefix("*** Move to: ")
        {
            let to = validate_patch_path(to)?;
            self.index += 1;
            self.changed_paths.push(to.clone());
            self.operations
                .push(ProjectOperation::FileRename { from: path, to });
            true
        } else {
            false
        };
        if !has_patches && !moved {
            return Err(RepositoryError::Invalid);
        }
        Ok(())
    }

    fn parse_hunk(&mut self) -> Result<ExactPatch, RepositoryError> {
        let mut old = Vec::new();
        let mut new = Vec::new();
        let mut changed = false;
        while self.index + 1 < self.lines.len() {
            let line = self.lines[self.index];
            if line.starts_with("@@") || line.starts_with("*** ") {
                break;
            }
            let (prefix, text) = line.split_at_checked(1).ok_or(RepositoryError::Invalid)?;
            match prefix {
                " " => {
                    old.push(text);
                    new.push(text);
                }
                "-" => {
                    old.push(text);
                    changed = true;
                }
                "+" => {
                    new.push(text);
                    changed = true;
                }
                _ => return Err(RepositoryError::Invalid),
            }
            self.index += 1;
        }
        let old = old.join("\n");
        let new = new.join("\n");
        if !changed || old.is_empty() || old == new {
            return Err(RepositoryError::Invalid);
        }
        Ok(ExactPatch { old, new })
    }

    fn at_section_boundary(&self) -> bool {
        self.index + 1 >= self.lines.len() || self.lines[self.index].starts_with("*** ")
    }
}

fn valid_hunk_header(header: &str) -> bool {
    header == "@@"
        || header
            .strip_prefix("@@ ")
            .is_some_and(|label| label.chars().any(|character| !character.is_whitespace()))
}

fn validate_patch_path(path: &str) -> Result<String, RepositoryError> {
    normalize_user_path(path)
}

fn validate_project_read_path(path: &str) -> Result<String, RepositoryError> {
    if path == AGENTS_PATH {
        Ok(path.to_owned())
    } else {
        normalize_user_path(path)
    }
}

async fn load_project(
    repository: &Repository,
    model_id: &str,
    revision: Option<&str>,
) -> Result<(ModelRecord, ProjectBundle, String), RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let actual_revision = revision
        .unwrap_or(&model.desired_source_revision)
        .to_owned();
    match repository.get_project(model_id, &actual_revision).await {
        Ok(project) => Ok((model, project, actual_revision)),
        Err(RepositoryError::NotFound) if revision.is_none() => Err(RepositoryError::Corrupt),
        Err(error) => Err(error),
    }
}

fn file_index(files: &[crate::model::project::ProjectFile]) -> Vec<Value> {
    files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "size_bytes": file.content.len(),
                "sha256": hex_digest(Sha256::digest(file.content.as_bytes()))
            })
        })
        .collect()
}

fn compile_glob(pattern: &str) -> Result<globset::GlobMatcher, RepositoryError> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES || !pattern.is_ascii() {
        return Err(RepositoryError::Invalid);
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .map_err(|_| RepositoryError::Invalid)
        .map(|glob| glob.compile_matcher())
}

const fn validate_read_bounds(offset: usize, limit: usize) -> Result<(), RepositoryError> {
    if offset == 0 || limit == 0 || limit > MAX_READ_LIMIT {
        return Err(RepositoryError::Invalid);
    }
    Ok(())
}

struct LineRead {
    content: String,
    total_lines: usize,
    truncated: bool,
}

fn read_lines(content: &str, offset: usize, limit: usize) -> Result<LineRead, RepositoryError> {
    let lines = logical_lines(content).collect::<Vec<_>>();
    if offset > lines.len().saturating_add(1) {
        return Err(RepositoryError::Invalid);
    }
    let start = offset - 1;
    let end = start.saturating_add(limit).min(lines.len());
    Ok(LineRead {
        content: lines[start..end].concat(),
        total_lines: lines.len(),
        truncated: end < lines.len(),
    })
}

fn logical_lines(content: &str) -> impl Iterator<Item = &str> {
    content.split_inclusive('\n')
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        model::{
            library::LibraryRelease,
            project::{ProjectEdit, ProjectFile},
        },
        storage::InMemoryObjectStore,
    };

    #[test]
    fn patch_parser_maps_ordered_add_update_move_and_delete() {
        let parsed = parse_patch(
            "*** Begin Patch\n*** Add File: empty.txt\n*** Add File: note.txt\n+one\n+two\n*** Update File: main.py\n@@ function\n old\n-value\n+changed\n*** Move to: src/main.py\n*** Delete File: old.py\n*** End Patch",
        )
        .expect("patch");
        assert_eq!(parsed.operations.len(), 5);
        assert_eq!(
            parsed.operations[1],
            ProjectOperation::FileAdd {
                path: "note.txt".to_owned(),
                content: "one\ntwo".to_owned()
            }
        );
        assert_eq!(
            parsed.changed_paths,
            ["empty.txt", "note.txt", "main.py", "src/main.py", "old.py"]
        );
    }

    #[test]
    fn patch_parser_supports_rename_only_updates() {
        let parsed = parse_patch(
            "*** Begin Patch\n*** Update File: old.py\n*** Move to: new.py\n*** End Patch",
        )
        .expect("rename patch");
        assert_eq!(
            parsed.operations,
            vec![ProjectOperation::FileRename {
                from: "old.py".to_owned(),
                to: "new.py".to_owned(),
            }]
        );
        assert_eq!(parsed.changed_paths, vec!["old.py", "new.py"]);
    }

    #[test]
    fn patch_parser_preserves_file_final_newlines() {
        let parsed =
            parse_patch("*** Begin Patch\n*** Add File: newline.txt\n+value\n+\n*** End Patch\n")
                .expect("newline patch");
        assert_eq!(
            parsed.operations,
            vec![ProjectOperation::FileAdd {
                path: "newline.txt".to_owned(),
                content: "value\n".to_owned(),
            }]
        );
    }

    #[test]
    fn patch_parser_rejects_malformed_protected_noop_and_oversized_patches() {
        for patch in [
            "*** Begin Patch\n*** Update File: AGENTS.md\n@@\n-old\n+new\n*** End Patch",
            "*** Begin Patch\n*** Update File: main.py\n@@\n same\n*** End Patch",
            "*** Begin Patch\n*** Add File: x\nunprefixed\n*** End Patch",
            "*** Begin Patch\n*** Unknown: x\n*** End Patch",
            "*** Begin Patch\n*** Add File: x\n+binary\0data\n*** End Patch",
            "*** Begin Patch\n*** End Patch",
            "*** Begin Patch\n*** Update File: x\n@@ \n-old\n+new\n*** End Patch",
            "*** Begin Patch\n*** Update File: x\n@@   \n-old\n+new\n*** End Patch",
            "*** Begin Patch\n*** Update File: x\n@@ \t\t\n-old\n+new\n*** End Patch",
            "*** Begin Patch\n*** Update File: x\n@@\tlabel\n-old\n+new\n*** End Patch",
        ] {
            assert_eq!(parse_patch(patch).err(), Some(RepositoryError::Invalid));
        }
        assert_eq!(
            parse_patch(&"x".repeat(MAX_PATCH_BYTES + 1)).err(),
            Some(RepositoryError::Invalid)
        );
    }

    #[test]
    fn glob_syntax_keeps_component_and_recursive_semantics() {
        let component = compile_glob("src/*.py").expect("component glob");
        assert!(component.is_match("src/a.py"));
        assert!(!component.is_match("src/nested/a.py"));

        let question = compile_glob("src/file?.py").expect("question glob");
        assert!(question.is_match("src/file1.py"));
        assert!(!question.is_match("src/file12.py"));

        let class = compile_glob("src/file[0-9].py").expect("class glob");
        assert!(class.is_match("src/file7.py"));
        assert!(!class.is_match("src/filex.py"));

        let recursive = compile_glob("src/**/part.py").expect("recursive glob");
        assert!(recursive.is_match("src/one/part.py"));
        assert!(recursive.is_match("src/one/two/part.py"));
        assert!(!recursive.is_match("other/one/part.py"));
    }

    #[test]
    fn multibyte_runtime_limits_are_measured_in_utf8_bytes() {
        let exact_regex = "é".repeat(MAX_PATTERN_BYTES / 2);
        assert_eq!(exact_regex.len(), MAX_PATTERN_BYTES);
        assert!(Regex::new(&exact_regex).is_ok());
        let oversized_regex = format!("{exact_regex}é");
        assert!(oversized_regex.len() > MAX_PATTERN_BYTES);

        let prefix = "*** Begin Patch\n*** Add File: exact.txt\n+";
        let suffix = "\n*** End Patch";
        let remaining = MAX_PATCH_BYTES - prefix.len() - suffix.len();
        let payload = format!(
            "{}{}",
            "é".repeat(remaining / 2),
            if remaining.is_multiple_of(2) { "" } else { "a" }
        );
        let exact_patch = format!("{prefix}{payload}{suffix}");
        assert_eq!(exact_patch.len(), MAX_PATCH_BYTES);
        assert!(parse_patch(&exact_patch).is_ok());
        assert_eq!(
            parse_patch(&format!("{exact_patch}é")).err(),
            Some(RepositoryError::Invalid)
        );
    }

    #[test]
    fn line_reads_and_utf8_truncation_are_exact_and_bounded() {
        let read = read_lines("one\ntwo\nthree", 2, 1).expect("read");
        assert_eq!(read.content, "two\n");
        assert_eq!(read.total_lines, 3);
        assert!(read.truncated);
        assert!(read_lines("", 1, 1).expect("empty").content.is_empty());
        assert_eq!(
            read_lines("one", 3, 1).err(),
            Some(RepositoryError::Invalid)
        );
        assert_eq!(truncate_utf8("éé", 3), ("é", true));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn model_workspace_opens_guidance_and_reads_exact_revisions_with_bounds() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "main.py".to_owned(),
                        content: "first\nmatch alpha\nmatch beta".to_owned(),
                    },
                    ProjectFile {
                        path: "notes.txt".to_owned(),
                        content: "private body".to_owned(),
                    },
                    ProjectFile {
                        path: "src/deep/nested.py".to_owned(),
                        content: "nested match".to_owned(),
                    },
                ],
                "main.py".to_owned(),
                Vec::new(),
                "Read guidance first.",
            )
            .await
            .expect("create project");
        let opened = model_open(&repository, "part").await.expect("open");
        assert_eq!(opened["revision"], created.desired_source_revision);
        assert!(
            opened["agents_md"]
                .as_str()
                .unwrap()
                .contains("Read guidance first.")
        );
        assert!(!opened.to_string().contains("private body"));
        assert_eq!(opened["files"].as_array().map(Vec::len), Some(4));
        assert!(
            opened["files"][0]["sha256"]
                .as_str()
                .is_some_and(|value| value.len() == 64)
        );

        let edited = repository
            .edit_project(
                "part",
                &created.desired_source_revision,
                None,
                Some(
                    &ProjectEdit::new(vec![ProjectOperation::FilePatch {
                        path: "main.py".to_owned(),
                        patches: vec![ExactPatch {
                            old: "first".to_owned(),
                            new: "second".to_owned(),
                        }],
                    }])
                    .expect("edit"),
                ),
            )
            .await
            .expect("edit project");
        let desired = model_read(
            &repository,
            &ModelReadInput {
                model_id: "part".to_owned(),
                path: "main.py".to_owned(),
                revision: None,
                offset: 1,
                limit: 1,
            },
        )
        .await
        .expect("desired read");
        assert_eq!(desired["revision"], edited.record.desired_source_revision);
        assert_eq!(desired["content"], "second\n");
        assert_eq!(desired["truncated"], true);
        let exact = model_read(
            &repository,
            &ModelReadInput {
                model_id: "part".to_owned(),
                path: "main.py".to_owned(),
                revision: Some(created.desired_source_revision.clone()),
                offset: 1,
                limit: MAX_READ_LIMIT,
            },
        )
        .await
        .expect("exact old read");
        assert_eq!(exact["revision"], created.desired_source_revision);
        assert!(exact["content"].as_str().unwrap().starts_with("first\n"));

        let globbed = model_glob(&repository, "part", "*.py", None)
            .await
            .expect("glob");
        assert_eq!(globbed["paths"], json!(["main.py"]));
        assert_eq!(globbed["truncated"], false);
        assert_eq!(
            model_glob(
                &repository,
                "part",
                &"x".repeat(MAX_PATTERN_BYTES + 1),
                None
            )
            .await
            .err(),
            Some(RepositoryError::Invalid)
        );

        let matches = model_grep(
            &repository,
            &ModelGrepInput {
                model_id: "part".to_owned(),
                pattern: "match".to_owned(),
                include: Some("*.py".to_owned()),
                revision: None,
                limit: 1,
            },
        )
        .await
        .expect("grep");
        assert_eq!(matches["matches"].as_array().map(Vec::len), Some(1));
        assert_eq!(matches["matches"][0]["line"], 2);
        assert_eq!(matches["truncated"], true);
        let recursive_matches = model_grep(
            &repository,
            &ModelGrepInput {
                model_id: "part".to_owned(),
                pattern: "nested".to_owned(),
                include: Some("src/**/*.py".to_owned()),
                revision: None,
                limit: DEFAULT_GREP_LIMIT,
            },
        )
        .await
        .expect("recursive include grep");
        assert_eq!(
            recursive_matches["matches"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            recursive_matches["matches"][0]["path"],
            "src/deep/nested.py"
        );

        let exact_multibyte = "é".repeat(MAX_PATTERN_BYTES / 2);
        assert!(
            model_grep(
                &repository,
                &ModelGrepInput {
                    model_id: "part".to_owned(),
                    pattern: exact_multibyte.clone(),
                    include: None,
                    revision: None,
                    limit: DEFAULT_GREP_LIMIT,
                },
            )
            .await
            .is_ok()
        );
        assert_eq!(
            model_grep(
                &repository,
                &ModelGrepInput {
                    model_id: "part".to_owned(),
                    pattern: format!("{exact_multibyte}é"),
                    include: None,
                    revision: None,
                    limit: DEFAULT_GREP_LIMIT,
                },
            )
            .await
            .err(),
            Some(RepositoryError::Invalid)
        );
        assert_eq!(
            model_grep(
                &repository,
                &ModelGrepInput {
                    model_id: "part".to_owned(),
                    pattern: "(".to_owned(),
                    include: None,
                    revision: None,
                    limit: DEFAULT_GREP_LIMIT,
                },
            )
            .await
            .err(),
            Some(RepositoryError::Invalid)
        );
    }

    #[tokio::test]
    async fn library_workspace_indexes_and_reads_immutable_release_files() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let release = LibraryRelease::new(
            "gears".to_owned(),
            Version::new(1, 2, 3),
            vec![ProjectFile {
                path: "faktory_shared/gears/__init__.py".to_owned(),
                content: "one\ntwo\nthree".to_owned(),
            }],
            "Use the public API.".to_owned(),
            vec![ProjectFile {
                path: "docs/guide.md".to_owned(),
                content: "Guide body".to_owned(),
            }],
        )
        .expect("release");
        let digest = release.digest.clone();
        repository.publish_library(release).await.expect("publish");

        let opened = library_open(&repository, "gears", &Version::new(1, 2, 3))
            .await
            .expect("open library");
        assert_eq!(opened["release_sha256"], digest);
        assert_eq!(opened["guidance"], "Use the public API.");
        assert!(!opened.to_string().contains("Guide body"));
        let read = library_read(
            &repository,
            &LibraryReadInput {
                name: "gears".to_owned(),
                version: Version::new(1, 2, 3),
                path: "faktory_shared/gears/__init__.py".to_owned(),
                offset: 2,
                limit: 1,
            },
        )
        .await
        .expect("read library");
        assert_eq!(read["release_sha256"], digest);
        assert_eq!(read["content"], "two\n");
        assert_eq!(read["total_lines"], 3);
        assert_eq!(read["truncated"], true);
    }
}
