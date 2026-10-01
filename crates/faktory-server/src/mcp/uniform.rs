//! Resource-first MCP facade over canonical projects, never filesystem access.

use mcp::{
    McpResourceList, McpResourceResult, McpResourceTemplateList, McpToolList,
    server::{BoxFuture, ServerCapabilities, ServerHandler, ServerInfo},
};

use super::*;
use crate::model::{
    project::{MAX_PROJECT_FILE_BYTES, normalize_user_path},
    validate_model_id,
};

impl ServerHandler for FaktoryMcp {
    fn server_info(&self) -> ServerInfo {
        ServerInfo::new("faktory", "0.1.0")
    }

    fn capabilities(&self) -> ServerCapabilities {
        ServerCapabilities::new().tools().resources().skills()
    }

    fn skill_catalog(&self) -> Option<Arc<mcp::skills::SkillCatalog>> {
        Some(self.catalog.clone())
    }

    fn list_tools(
        &self,
        cursor: Option<String>,
        _: ServerContext,
    ) -> BoxFuture<ServerResult<McpToolList>> {
        Box::pin(async move {
            reject_cursor(cursor)?;
            Ok(McpToolList {
                tools: definitions(),
                next_cursor: None,
            })
        })
    }

    fn call_tool(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> BoxFuture<ServerResult<McpToolResult>> {
        let server = self.clone();
        Box::pin(async move {
            if !["create", "edit", "destroy", "execute", "query"].contains(&call.name.as_str()) {
                return Err(ServerError::method_not_found("unknown tool"));
            }
            if call.name == "edit" {
                let input: TextEditInput = parse(call)?;
                return Ok(match server.edit_text(input).await {
                    Ok(model) => result(json!({"model":model,"uri":model_uri(&model.id)})),
                    Err(error) => tool_error(error),
                });
            }
            let envelope: ActionInput = parse(call.clone())?;
            let action = envelope.action;
            let arguments = envelope.input;
            if !arguments.is_object() {
                return Err(ServerError::invalid_params("input must be an object"));
            }
            let input = McpToolCall {
                arguments,
                ..call.clone()
            };
            server.action(&call.name, &action, input).await
        })
    }

    fn list_resources(
        &self,
        cursor: Option<String>,
        _: ServerContext,
    ) -> BoxFuture<ServerResult<McpResourceList>> {
        Box::pin(async move {
            reject_cursor(cursor)?;
            Ok(McpResourceList {
                resources: vec![mcp::parse_resource_definition(&json!({
                    "uri":"faktory://models", "name":"Models", "mimeType":"application/json",
                    "description":"Model collection with canonical project, release, view, and stored-image links."
                })).expect("static resource definition")], next_cursor: None,
            })
        })
    }

    #[allow(clippy::too_many_lines)]
    fn list_resource_templates(
        &self,
        cursor: Option<String>,
        _: ServerContext,
    ) -> BoxFuture<ServerResult<McpResourceTemplateList>> {
        Box::pin(async move {
            reject_cursor(cursor)?;
            let resource_templates = [
                (
                    "faktory://models/{model_id}",
                    "Model metadata",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/project",
                    "Complete desired project",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/open",
                    "Project summary and guidance",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/files",
                    "File index",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/files/{path}",
                    "Desired file",
                    "text/plain",
                ),
                (
                    "faktory://models/{model_id}/revisions/{revision}/files/{path}",
                    "Immutable file",
                    "text/plain",
                ),
                (
                    "faktory://models/{model_id}/hints",
                    "Editable hints body",
                    "text/plain",
                ),
                (
                    "faktory://models/{model_id}/releases",
                    "Release collection",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/releases/{version}",
                    "Release metadata and exact closure",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/views",
                    "View collection",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/views/{view_id}",
                    "Saved view",
                    "application/json",
                ),
                (
                    "faktory://models/{model_id}/views/{view_id}/image",
                    "Existing saved-view image; never renders",
                    "image/png",
                ),
                (
                    "faktory://models/{model_id}/images/{output_id}/{style}/{projection}",
                    "Stored last-successful projection",
                    "image/png",
                ),
            ]
            .into_iter()
            .map(|(uri, name, mime)| {
                mcp::parse_resource_template_definition(
                    &json!({"uriTemplate":uri,"name":name,"mimeType":mime}),
                )
                .expect("static template definition")
            })
            .collect();
            Ok(McpResourceTemplateList {
                resource_templates,
                next_cursor: None,
            })
        })
    }

    fn read_resource(
        &self,
        uri: String,
        _: ServerContext,
    ) -> BoxFuture<ServerResult<McpResourceResult>> {
        let server = self.clone();
        Box::pin(async move {
            if let Some(resource) = server.catalog.read(&uri) {
                return Ok(resource);
            }
            server
                .read(&uri)
                .await
                .map_err(|error| resource_error(error, &uri))
        })
    }
}

fn reject_cursor(cursor: Option<String>) -> ServerResult<()> {
    if cursor.is_some() {
        Err(ServerError::invalid_params("invalid cursor"))
    } else {
        Ok(())
    }
}

fn resource_error(error: RepositoryError, uri: &str) -> ServerError {
    match error {
        RepositoryError::Invalid | RepositoryError::NotFound => {
            ServerError::resource_not_found(uri)
        }
        RepositoryError::Conflict => {
            ServerError::invalid_params("Resource changed concurrently; read again.")
        }
        RepositoryError::Corrupt => ServerError::internal("Stored state is invalid."),
        RepositoryError::Unavailable => {
            ServerError::internal("Faktory is temporarily unavailable.")
        }
    }
}

pub(super) fn model_uri(id: &str) -> String {
    format!("faktory://models/{id}")
}

fn encoded_path(path: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("write String");
        }
    }
    encoded
}

fn decoded_path(encoded: &str) -> Result<String, RepositoryError> {
    let mut bytes = Vec::new();
    let mut remaining = encoded.as_bytes();
    while let Some((&first, tail)) = remaining.split_first() {
        if first == b'%' {
            let hex = tail.get(..2).ok_or(RepositoryError::Invalid)?;
            let text = std::str::from_utf8(hex).map_err(|_| RepositoryError::Invalid)?;
            bytes.push(u8::from_str_radix(text, 16).map_err(|_| RepositoryError::Invalid)?);
            remaining = &tail[2..];
        } else {
            bytes.push(first);
            remaining = tail;
        }
    }
    let path = String::from_utf8(bytes).map_err(|_| RepositoryError::Invalid)?;
    if encoded_path(&path) != encoded
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(RepositoryError::Invalid);
    }
    if path != "AGENTS.md" {
        normalize_user_path(&path)?;
    }
    Ok(path)
}

fn parts(uri: &str) -> Result<Vec<&str>, RepositoryError> {
    let rest = uri
        .strip_prefix("faktory://models/")
        .ok_or(RepositoryError::Invalid)?;
    if rest.contains(['?', '#']) || rest.ends_with('/') {
        return Err(RepositoryError::Invalid);
    }
    let parts: Vec<_> = rest.split('/').collect();
    validate_model_id(parts[0])?;
    Ok(parts)
}

fn text_resource(uri: &str, mime: &str, text: String, metadata: Value) -> McpResourceResult {
    McpResourceResult {
        raw: json!({"contents":[{"uri":uri,"mimeType":mime,"text":text}],"_meta":metadata}),
    }
}

fn json_resource(uri: &str, value: Value) -> McpResourceResult {
    text_resource(uri, "application/json", value.to_string(), json!({}))
}

async fn desired_project(
    repository: &Repository,
    id: &str,
    revision: &str,
) -> Result<crate::model::project::ProjectBundle, RepositoryError> {
    repository.get_project(id, revision).await.map_err(|error| {
        if error == RepositoryError::NotFound {
            RepositoryError::Corrupt
        } else {
            error
        }
    })
}

fn image_resource(uri: &str, image: &str, metadata: Value) -> McpResourceResult {
    McpResourceResult {
        raw: json!({"contents":[
            {"uri":uri,"mimeType":"image/png","blob":image},
            {"uri":uri,"mimeType":"application/json","text":metadata.to_string()}
        ]}),
    }
}

fn image_links(model: &ModelRecord) -> Vec<String> {
    let mut links = Vec::new();
    for output in model.effective_outputs() {
        for projection in [
            "isometric",
            "front",
            "back",
            "left",
            "right",
            "top",
            "bottom",
        ] {
            for style in ["technical", "shaded"] {
                if style == "technical" || output.primary {
                    links.push(format!(
                        "{}/images/{}/{style}/{projection}",
                        model_uri(&model.id),
                        output.output_id
                    ));
                }
            }
        }
    }
    links
}

impl FaktoryMcp {
    #[allow(clippy::too_many_lines)]
    async fn read(&self, uri: &str) -> Result<McpResourceResult, RepositoryError> {
        if uri == "faktory://models" {
            let models = self.repository.list_models().await?;
            return Ok(json_resource(
                uri,
                json!({"models":models.iter().map(|model| json!({"model":model,"uri":model_uri(&model.id)})).collect::<Vec<_>>()}),
            ));
        }
        let segments = parts(uri)?;
        let id = segments[0];
        let model = self.repository.get_model(id).await?.record;
        let base = model_uri(id);
        let desired = model.desired_source_revision.clone();
        let metadata = json!({"model_id":id,"revision":desired});
        let value = match &segments[1..] {
            [] => {
                json!({"model":model,"uri":base,"project_uri":format!("{base}/project"),"open_uri":format!("{base}/open"),"files_uri":format!("{base}/files"),"hints_uri":format!("{base}/hints"),"releases_uri":format!("{base}/releases"),"views_uri":format!("{base}/views"),"images":image_links(&model)})
            }
            ["project"] => {
                let project = desired_project(&self.repository, id, &desired).await?;
                json!({"model":model,"project":project,"revision":desired,"files_uri":format!("{base}/files")})
            }
            ["open"] => workspace::model_open(&self.repository, id).await?,
            ["files"] => {
                let project = desired_project(&self.repository, id, &desired).await?;
                let mut files = workspace::file_index(&project.files);
                for file in &mut files {
                    let path = file["path"]
                        .as_str()
                        .ok_or(RepositoryError::Corrupt)?
                        .to_owned();
                    file["uri"] = json!(format!(
                        "{base}/revisions/{desired}/files/{}",
                        encoded_path(&path)
                    ));
                    if path != "AGENTS.md" {
                        file["edit_uri"] = json!(format!("{base}/files/{}", encoded_path(&path)));
                    }
                }
                json!({"model_id":id,"revision":desired,"files":files})
            }
            ["hints"] => {
                let project = desired_project(&self.repository, id, &desired).await?;
                return Ok(text_resource(
                    uri,
                    "text/plain",
                    project.hints()?.to_owned(),
                    metadata,
                ));
            }
            ["files", path] => {
                return self
                    .file_resource(uri, id, &desired, &decoded_path(path)?)
                    .await;
            }
            ["revisions", revision, "files", path] => {
                return self
                    .file_resource(uri, id, revision, &decoded_path(path)?)
                    .await;
            }
            ["releases"] => {
                json!({"model_id":id,"releases":self.repository.list_model_releases(id).await?.iter().map(|release| json!({"release":model_release_value(release),"uri":format!("{base}/releases/{}",release.version)})).collect::<Vec<_>>()})
            }
            ["releases", version] => {
                let input = ModelReleaseGetInput {
                    model_id: id.to_owned(),
                    version: Version::parse(version).map_err(|_| RepositoryError::Invalid)?,
                };
                let mut value = model_release_get_value(&self.repository, &input).await?;
                let revision = value["release"]["project_revision"]
                    .as_str()
                    .ok_or(RepositoryError::Corrupt)?
                    .to_owned();
                if let Some(files) = value["files"].as_array_mut() {
                    for file in files {
                        if let Some(path) = file["path"].as_str() {
                            file["uri"] = json!(format!(
                                "{base}/revisions/{revision}/files/{}",
                                encoded_path(path)
                            ));
                        }
                    }
                }
                value
            }
            ["views"] => {
                json!({"views":self.repository.list_views(id).await?.iter().map(|view| json!({"view":view,"uri":format!("{base}/views/{}",view.id),"image_uri":format!("{base}/views/{}/image",view.id)})).collect::<Vec<_>>()})
            }
            ["views", view] => json!({"view":self.repository.get_view(id,view).await?.record}),
            ["views", view, "image"] => {
                let view = self.repository.get_view(id, view).await?.record;
                let revision = &model.current_successful_source_revision;
                if revision.is_empty() {
                    return Err(RepositoryError::NotFound);
                }
                let output = model.primary_output()?;
                let identity = ViewRenderIdentity {
                    revision: revision.clone(),
                    output_id: output.output_id.clone(),
                    view_id: view.id.clone(),
                    view_etag: view.etag.clone(),
                };
                let image = self.repository.cached_view_image(id, &identity).await?;
                validate_visual_png(&image).map_err(|_| RepositoryError::Corrupt)?;
                let current = self.repository.get_model(id).await?.record;
                let current_view = self.repository.get_view(id, &view.id).await?.record;
                if current.current_successful_source_revision != *revision
                    || current_view.etag != view.etag
                    || current.primary_output()?.output_id != identity.output_id
                {
                    return Err(RepositoryError::Conflict);
                }
                return Ok(image_resource(
                    uri,
                    &BASE64.encode(image),
                    json!({"model_id":id,"output_id":output.output_id,"output_role":output.role,"primary":true,"view_id":view.id,"view_etag":view.etag,"desired_revision":current.desired_source_revision,"rendered_revision":revision,"render_state":current.render_state,"stale":current.desired_source_revision != *revision,"style":"shaded","recipe":VISUAL_RECIPE,"width":PROJECTION_WIDTH,"height":PROJECTION_HEIGHT,"mime_type":"image/png"}),
                ));
            }
            ["images", output, style, projection] => {
                let projection = serde_json::from_value(json!(projection))
                    .map_err(|_| RepositoryError::Invalid)?;
                let style =
                    serde_json::from_value(json!(style)).map_err(|_| RepositoryError::Invalid)?;
                let image =
                    inspect_model(&self.repository, id, Some(output), projection, style).await?;
                return Ok(image_resource(
                    uri,
                    image.raw["content"][1]["data"]
                        .as_str()
                        .ok_or(RepositoryError::Corrupt)?,
                    image.raw["structuredContent"]["metadata"].clone(),
                ));
            }
            _ => return Err(RepositoryError::NotFound),
        };
        let mut value = value;
        if segments.get(1) == Some(&"open") {
            value["files_uri"] = json!(format!("{base}/files"));
            value["hints_uri"] = json!(format!("{base}/hints"));
            let revision = value["revision"]
                .as_str()
                .ok_or(RepositoryError::Corrupt)?
                .to_owned();
            if let Some(files) = value["files"].as_array_mut() {
                for file in files {
                    if let Some(path) = file["path"].as_str() {
                        file["uri"] = json!(format!(
                            "{base}/revisions/{revision}/files/{}",
                            encoded_path(path)
                        ));
                    }
                }
            }
        }
        Ok(json_resource(uri, value))
    }

    async fn file_resource(
        &self,
        uri: &str,
        id: &str,
        revision: &str,
        path: &str,
    ) -> Result<McpResourceResult, RepositoryError> {
        let desired = parts(uri)?.get(1) == Some(&"files");
        let project = if desired {
            desired_project(&self.repository, id, revision).await?
        } else {
            self.repository.get_project(id, revision).await?
        };
        let file = project
            .files
            .iter()
            .find(|file| file.path == path)
            .ok_or(RepositoryError::NotFound)?;
        Ok(text_resource(
            uri,
            "text/plain",
            file.content.clone(),
            json!({"model_id":id,"revision":revision,"path":path}),
        ))
    }

    pub(super) async fn edit_text(
        &self,
        input: TextEditInput,
    ) -> Result<ModelRecord, RepositoryError> {
        let segments = parts(&input.uri)?;
        let id = segments[0];
        let model = self.repository.get_model(id).await?.record;
        if model.desired_source_revision != input.expected_revision {
            return Err(RepositoryError::Conflict);
        }
        let project = desired_project(&self.repository, id, &input.expected_revision).await?;
        let operation = match &segments[1..] {
            ["hints"] => {
                let old = project.hints()?.to_owned();
                let new = apply_edits(&old, &input.edits)?;
                ProjectOperation::HintsTextSet { content: new }
            }
            ["files", path] => {
                let path = decoded_path(path)?;
                normalize_user_path(&path)?;
                let old = project
                    .files
                    .iter()
                    .find(|file| file.path == path)
                    .ok_or(RepositoryError::NotFound)?
                    .content
                    .clone();
                let new = apply_edits(&old, &input.edits)?;
                ProjectOperation::FileTextSet { path, content: new }
            }
            _ => return Err(RepositoryError::Invalid),
        };
        edit_and_schedule(
            self.repository.clone(),
            self.renders.clone(),
            EditInput {
                model_id: id.to_owned(),
                expected_revision: input.expected_revision,
                name: None,
                operations: Some(vec![operation]),
            },
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn action(
        &self,
        tool: &str,
        action: &str,
        call: McpToolCall,
    ) -> ServerResult<McpToolResult> {
        let output = match (tool, action) {
            ("create", "model.create") => {
                let input = parse(call)?;
                create_and_schedule(self.repository.clone(), self.renders.clone(), input)
                    .await
                    .map(|model| json!({"uri":model_uri(&model.id),"model":model}))
            }
            ("create", "file.create")
            | ("destroy", "file.destroy")
            | ("execute", "file.rename" | "entrypoint.set" | "dependencies.set") => {
                let fields: &[&str] = match action {
                    "file.create" => &["model_id", "expected_revision", "path", "content"],
                    "file.destroy" | "entrypoint.set" => &["model_id", "expected_revision", "path"],
                    "file.rename" => &["model_id", "expected_revision", "from", "to"],
                    "dependencies.set" => &["model_id", "expected_revision", "requirements"],
                    _ => unreachable!(),
                };
                action_fields(&call, fields)?;
                let input: ProjectActionInput = parse(call)?;
                let operation = input.operation(action)?;
                edit_and_schedule(
                    self.repository.clone(),
                    self.renders.clone(),
                    EditInput {
                        model_id: input.model_id,
                        expected_revision: input.expected_revision,
                        name: None,
                        operations: Some(vec![operation]),
                    },
                )
                .await
                .map(|model| json!({"uri":model_uri(&model.id),"model":model}))
            }
            ("execute", "model.set-name") => {
                let input: NameInput = parse(call)?;
                edit_and_schedule(
                    self.repository.clone(),
                    self.renders.clone(),
                    EditInput {
                        model_id: input.model_id,
                        expected_revision: input.expected_revision,
                        name: Some(input.name),
                        operations: None,
                    },
                )
                .await
                .map(|model| json!({"model":model}))
            }
            ("execute", "model.release.publish") => {
                publish_and_rollout(self.repository.clone(), self.renders.clone(), parse(call)?)
                    .await
            }
            ("execute", "model.render.retry") => {
                retry_and_schedule(self.repository.clone(), self.renders.clone(), parse(call)?)
                    .await
                    .map(|model| json!({"model":model}))
            }
            ("create", "view.create") | ("execute", "view.update") => {
                if tool == "create" {
                    action_fields(&call, &["model_id", "view"])?;
                }
                let input: PutViewInput = parse(call)?;
                let valid = if tool == "create" {
                    input.view.id.is_empty() && input.expected_etag.is_none()
                } else {
                    !input.view.id.is_empty() && input.expected_etag.is_some()
                };
                if !valid {
                    return Err(ServerError::invalid_params(
                        "creation and guarded update are distinct",
                    ));
                }
                self.repository.put_view(&input.model_id,input.view.into_proto(),input.expected_etag.as_deref()).await.map(|view| json!({"uri":format!("{}/views/{}",model_uri(&input.model_id),view.id),"view":view}))
            }
            ("destroy", "view.destroy") => {
                let input: DeleteViewInput = parse(call)?;
                self.repository
                    .delete_view(&input.model_id, &input.view_id, &input.expected_etag)
                    .await
                    .map(|()| json!({"deleted":true}))
            }
            ("execute", "view.set-default") => {
                let input: ViewIdInput = parse(call)?;
                self.repository
                    .set_default_view(&input.model_id, &input.view_id)
                    .await
                    .map(|model| json!({"model":model}))
            }
            ("query", "model.glob") => {
                let input: ModelGlobInput = parse(call)?;
                workspace::model_glob(
                    &self.repository,
                    &input.model_id,
                    &input.pattern,
                    input.revision.as_deref(),
                )
                .await
            }
            ("query", "model.grep") => workspace::model_grep(&self.repository, &parse(call)?).await,
            ("query", "view.render") => {
                let input: ViewIdInput = parse(call)?;
                return Ok(
                    match inspect_view(
                        &self.repository,
                        &self.visual,
                        &input.model_id,
                        &input.view_id,
                    )
                    .await
                    {
                        Ok(image) => image,
                        Err(error) => tool_error(error),
                    },
                );
            }
            _ => return Err(ServerError::invalid_params("unknown action for tool")),
        };
        Ok(match output {
            Ok(value) => result(value),
            Err(error) => tool_error(error),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TextEditInput {
    uri: String,
    expected_revision: String,
    edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum TextEdit {
    Replace {
        old_text: String,
        new_text: String,
    },
    Insert {
        text: String,
        placement: Placement,
        #[serde(default, deserialize_with = "optional_string")]
        anchor: Option<String>,
    },
}

fn optional_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Placement {
    Start,
    End,
    Before,
    After,
}

fn unique_match(text: &str, needle: &str) -> Result<usize, RepositoryError> {
    if needle.is_empty() {
        return Err(RepositoryError::Invalid);
    }
    let mut positions = text
        .as_bytes()
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle.as_bytes()).then_some(index));
    let position = positions.next().ok_or(RepositoryError::Invalid)?;
    if positions.next().is_some() {
        return Err(RepositoryError::Invalid);
    }
    Ok(position)
}

fn apply_edits(old: &str, edits: &[TextEdit]) -> Result<String, RepositoryError> {
    if edits.is_empty() || edits.len() > 256 {
        return Err(RepositoryError::Invalid);
    }
    let mut value = old.to_owned();
    for edit in edits {
        match edit {
            TextEdit::Replace { old_text, new_text } => {
                let position = unique_match(&value, old_text)?;
                if old_text == new_text {
                    return Err(RepositoryError::Invalid);
                }
                value.replace_range(position..position + old_text.len(), new_text);
            }
            TextEdit::Insert {
                text,
                placement,
                anchor,
            } => {
                if text.is_empty() {
                    return Err(RepositoryError::Invalid);
                }
                let position = match placement {
                    Placement::Start | Placement::End => {
                        if anchor.is_some() {
                            return Err(RepositoryError::Invalid);
                        }
                        if matches!(placement, Placement::Start) {
                            0
                        } else {
                            value.len()
                        }
                    }
                    Placement::Before | Placement::After => {
                        let anchor = anchor.as_deref().ok_or(RepositoryError::Invalid)?;
                        let position = unique_match(&value, anchor)?;
                        position
                            + if matches!(placement, Placement::After) {
                                anchor.len()
                            } else {
                                0
                            }
                    }
                };
                value.insert_str(position, text);
            }
        }
        if value.len() > MAX_PROJECT_FILE_BYTES {
            return Err(RepositoryError::Invalid);
        }
    }
    if value == old {
        return Err(RepositoryError::Invalid);
    }
    Ok(value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameInput {
    model_id: String,
    expected_revision: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectActionInput {
    model_id: String,
    expected_revision: String,
    path: Option<String>,
    content: Option<String>,
    from: Option<String>,
    to: Option<String>,
    requirements: Option<Vec<DirectRequirement>>,
}

fn action_fields(call: &McpToolCall, allowed: &[&str]) -> ServerResult<()> {
    let object = call
        .arguments
        .as_object()
        .ok_or_else(|| ServerError::invalid_params("invalid arguments"))?;
    if object
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        return Err(ServerError::invalid_params("unknown action argument"));
    }
    Ok(())
}

impl ProjectActionInput {
    fn operation(&self, action: &str) -> ServerResult<ProjectOperation> {
        let required = |value: &Option<String>| {
            value
                .clone()
                .ok_or_else(|| ServerError::invalid_params("missing action argument"))
        };
        let valid = match action {
            "file.create" => {
                self.path.is_some()
                    && self.content.is_some()
                    && self.from.is_none()
                    && self.to.is_none()
                    && self.requirements.is_none()
            }
            "file.destroy" | "entrypoint.set" => {
                self.path.is_some()
                    && self.content.is_none()
                    && self.from.is_none()
                    && self.to.is_none()
                    && self.requirements.is_none()
            }
            "file.rename" => {
                self.path.is_none()
                    && self.content.is_none()
                    && self.from.is_some()
                    && self.to.is_some()
                    && self.requirements.is_none()
            }
            "dependencies.set" => {
                self.path.is_none()
                    && self.content.is_none()
                    && self.from.is_none()
                    && self.to.is_none()
                    && self.requirements.is_some()
            }
            _ => false,
        };
        if !valid {
            return Err(ServerError::invalid_params("invalid action arguments"));
        }
        Ok(match action {
            "file.create" => ProjectOperation::FileAdd {
                path: required(&self.path)?,
                content: required(&self.content)?,
            },
            "file.destroy" => ProjectOperation::FileDelete {
                path: required(&self.path)?,
            },
            "file.rename" => ProjectOperation::FileRename {
                from: required(&self.from)?,
                to: required(&self.to)?,
            },
            "entrypoint.set" => ProjectOperation::EntrypointSet {
                path: required(&self.path)?,
            },
            "dependencies.set" => ProjectOperation::DependenciesSet {
                requirements: self.requirements.clone().expect("validated requirements"),
            },
            _ => unreachable!("validated action"),
        })
    }
}

fn variant(action: &str, schema: Value) -> Value {
    object(
        json!({"action":{"const":action},"input":schema}),
        &["action", "input"],
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionInput {
    action: String,
    input: Value,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn project_action_schema(action: &str) -> Value {
    let mut properties = json!({"model_id":model_id_property(),"expected_revision":revision_schema("Exact desired project revision.")});
    let mut required = vec!["model_id", "expected_revision"];
    match action {
        "file.create" => {
            properties["path"] = project_path_schema();
            properties["content"] = json!({"type":"string"});
            required.extend(["path", "content"]);
        }
        "file.destroy" | "entrypoint.set" => {
            properties["path"] = project_path_schema();
            required.push("path");
        }
        "file.rename" => {
            properties["from"] = project_path_schema();
            properties["to"] = project_path_schema();
            required.extend(["from", "to"]);
        }
        "dependencies.set" => {
            properties["requirements"] =
                json!({"type":"array","maxItems":64,"items":dependency_schema()});
            required.push("requirements");
        }
        _ => unreachable!(),
    }
    variant(action, object(properties, &required))
}

#[allow(clippy::too_many_lines)]
pub(super) fn definitions() -> Vec<McpToolDefinition> {
    let view = json!({"type":"object","properties":{
        "id":{"type":"string","default":""},"name":{"type":"string"},
        "target":{"type":"array","minItems":3,"maxItems":3,"items":{"type":"number"}},
        "rotation":{"type":"array","minItems":4,"maxItems":4,"items":{"type":"number"}},
        "projection":{"enum":["PERSPECTIVE","ORTHOGRAPHIC"]},
        "distance":{"type":"number"},"field_of_view_degrees":{"type":"number"},"orthographic_scale":{"type":"number"}
    },"required":["name","target","rotation","projection","distance","field_of_view_degrees","orthographic_scale"],"additionalProperties":false});
    let mut create_view = view.clone();
    create_view["properties"]["id"] = json!({"const":""});
    let mut update_view = view;
    update_view["properties"]["id"] = json!({"type":"string","minLength":1,"maxLength":64});
    update_view["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("id"));
    let view_create = variant(
        "view.create",
        object(
            json!({"model_id":model_id_property(),"view":create_view}),
            &["model_id", "view"],
        ),
    );
    let view_update = variant(
        "view.update",
        object(
            json!({"model_id":model_id_property(),"view":update_view,"expected_etag":{"type":"string","minLength":1}}),
            &["model_id", "view", "expected_etag"],
        ),
    );
    let view_id = object(
        json!({"model_id":model_id_property(),"view_id":{"type":"string","minLength":1,"maxLength":64}}),
        &["model_id", "view_id"],
    );
    let mut view_destroy = view_id.clone();
    view_destroy["properties"]["expected_etag"] = json!({"type":"string","minLength":1});
    view_destroy["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("expected_etag"));
    let edit = object(
        json!({
            "uri":{"type":"string","description":"Canonical desired file or hints URI; generated AGENTS.md and immutable files are protected."},
            "expected_revision":revision_schema("Exact desired project revision guarding the whole atomic edit."),
            "edits":{"type":"array","minItems":1,"maxItems":256,"items":{"oneOf":[
                object(json!({"operation":{"const":"replace"},"old_text":{"type":"string","minLength":1},"new_text":{"type":"string"}}),&["operation","old_text","new_text"]),
                object(json!({"operation":{"const":"insert"},"text":{"type":"string","minLength":1},"placement":{"enum":["start","end"]}}),&["operation","text","placement"]),
                object(json!({"operation":{"const":"insert"},"text":{"type":"string","minLength":1},"placement":{"enum":["before","after"]},"anchor":{"type":"string","minLength":1}}),&["operation","text","placement","anchor"])
            ]}}
        }),
        &["uri", "expected_revision", "edits"],
    );
    vec![
        definition(
            "create",
            "Create a new model, caller file, or generated-ID saved view; never upsert.",
            false,
            json!({"type":"object","oneOf":[variant("model.create",model_create_definition().input_schema),project_action_schema("file.create"),view_create]}),
        ),
        definition(
            "edit",
            "Apply 1-256 ordered atomic text edits. Matches including overlaps must be unique; no-op edits are rejected. One source commit schedules one render.",
            false,
            edit,
        ),
        definition(
            "destroy",
            "Guarded removal of an existing caller file or saved view. Models and releases cannot be deleted.",
            false,
            json!({"type":"object","oneOf":[project_action_schema("file.destroy"),variant("view.destroy",view_destroy)]}),
        ),
        definition(
            "execute",
            "Change non-text project settings, saved views, retry rendering, or publish an immutable release.",
            false,
            json!({"type":"object","oneOf":[
                variant("model.set-name",object(json!({"model_id":model_id_property(),"expected_revision":revision_schema("Exact desired revision."),"name":{"type":"string","minLength":1}}),&["model_id","expected_revision","name"])),
                project_action_schema("file.rename"),project_action_schema("entrypoint.set"),project_action_schema("dependencies.set"),
                view_update,variant("view.set-default",view_id.clone()),variant("model.render.retry",model_retry_definition().input_schema),variant("model.release.publish",model_release_publish_definition().input_schema)
            ]}),
        ),
        definition(
            "query",
            "Exceptional bounded glob/grep search or on-demand saved-view rendering. Ordinary reads and stored images use resources.",
            true,
            json!({"type":"object","oneOf":[variant("model.glob",model_glob_definition().input_schema),variant("model.grep",model_grep_definition().input_schema),variant("view.render",view_id)]}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::tests::{named_view, rendered_output, valid_projection_png};
    use super::*;
    use crate::{
        render::RenderConfig,
        storage::{InMemoryObjectStore, ObjectStore as _, PutCondition},
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use bytes::Bytes;
    use std::time::Duration;
    use tower::ServiceExt as _;

    fn server(repository: &Repository) -> FaktoryMcp {
        let renders = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["unused".to_owned()],
                queue_capacity: 32,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 1024,
            },
        )
        .unwrap();
        FaktoryMcp::new(repository.clone(), renders).unwrap()
    }

    async fn rpc(server: &FaktoryMcp, method: &str, mut params: Value) -> Value {
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"}});
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", method);
        if let Some(name) = params["name"].as_str().or_else(|| params["uri"].as_str()) {
            request = request.header("mcp-name", name);
        }
        let request = request
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
            ))
            .unwrap();
        let response = server.clone().router().oneshot(request).await.unwrap();
        assert!([StatusCode::OK, StatusCode::BAD_REQUEST].contains(&response.status()));
        let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        serde_json::from_str(
            text.strip_prefix("data: ")
                .and_then(|value| value.strip_suffix("\n\n"))
                .unwrap_or(text),
        )
        .unwrap()
    }

    fn edits(value: Value) -> Vec<TextEdit> {
        serde_json::from_value(value).unwrap()
    }

    #[allow(clippy::too_many_lines)]
    async fn assert_historical_noops_do_not_commit_or_submit(
        repository: &Repository,
        store: &InMemoryObjectStore,
        server: &FaktoryMcp,
        historical: &crate::model::project::ProjectBundle,
    ) {
        use crate::model::{model_key, project::project_key};
        let before = repository.get_model("legacy").await.unwrap().record;
        let revision = &before.desired_source_revision;
        let model_object = store.get(&model_key("legacy")).await.unwrap();
        let project_object = store.get(&project_key("legacy", revision)).await.unwrap();
        let inventory = store.list("").await.unwrap();
        let mut changes = repository.subscribe();
        let arguments = [
            (
                "edit",
                json!({"uri":"faktory://models/legacy/hints","expected_revision":revision,"edits":[{"operation":"insert","text":"\n","placement":"end"}]}),
            ),
            (
                "edit",
                json!({"uri":"faktory://models/legacy/hints","expected_revision":revision,"edits":[{"operation":"replace","old_text":"Legacy hints","new_text":"\u{feff}Legacy hints\r\n"}]}),
            ),
            (
                "edit",
                json!({"uri":"faktory://models/legacy/files/main.py","expected_revision":revision,"edits":[{"operation":"replace","old_text":"result = 1","new_text":"\u{feff}result = 1"}]}),
            ),
            (
                "execute",
                json!({"action":"entrypoint.set","input":{"model_id":"legacy","expected_revision":revision,"path":"main.py"}}),
            ),
            (
                "execute",
                json!({"action":"dependencies.set","input":{"model_id":"legacy","expected_revision":revision,"requirements":historical.requirements}}),
            ),
        ];
        // Canonical validation still follows existing admission: a no-op may reserve capacity,
        // but its failed spawned commit must release the reservation without submitting a job.
        let mut reservations = Vec::new();
        for _ in 0..32 {
            reservations.push(server.renders.reserve().await.unwrap());
        }
        for (index, (name, arguments)) in arguments.into_iter().enumerate() {
            let task_server = server.clone();
            let mut request = tokio::spawn(async move {
                rpc(
                    &task_server,
                    "tools/call",
                    json!({"name":name,"arguments":arguments}),
                )
                .await
            });
            if index == 0 {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), &mut request)
                        .await
                        .is_err()
                );
                drop(reservations.pop());
            }
            let response = tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(repository.get_model("legacy").await.unwrap().record, before);
            let retained_model = store.get(&model_key("legacy")).await.unwrap();
            assert_eq!(retained_model.bytes, model_object.bytes);
            assert_eq!(retained_model.etag, model_object.etag);
            let retained_project = store.get(&project_key("legacy", revision)).await.unwrap();
            assert_eq!(retained_project.bytes, project_object.bytes);
            assert_eq!(retained_project.etag, project_object.etag);
            assert_eq!(store.list("").await.unwrap(), inventory);
            assert!(matches!(
                changes.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            let released = tokio::time::timeout(Duration::from_secs(1), server.renders.reserve())
                .await
                .unwrap()
                .unwrap();
            drop(released);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn ordered_text_edits_cover_insertions_unique_overlaps_and_noops() {
        let operations = edits(json!([
            {"operation":"insert","text":"first ","placement":"start"},
            {"operation":"replace","old_text":"body","new_text":"middle"},
            {"operation":"insert","text":"!","placement":"after","anchor":"middle"},
            {"operation":"insert","text":"<","placement":"before","anchor":"middle"},
            {"operation":"insert","text":" last","placement":"end"}
        ]));
        assert_eq!(
            apply_edits("body", &operations).unwrap(),
            "first <middle! last"
        );
        assert_eq!(
            apply_edits(
                "",
                &edits(json!([{"operation":"insert","text":"body","placement":"start"}]))
            )
            .unwrap(),
            "body"
        );
        for (old, edit) in [
            (
                "aaa",
                json!({"operation":"replace","old_text":"aa","new_text":"b"}),
            ),
            (
                "aaa",
                json!({"operation":"insert","text":"b","placement":"after","anchor":"aa"}),
            ),
            (
                "a",
                json!({"operation":"replace","old_text":"","new_text":"b"}),
            ),
            (
                "a",
                json!({"operation":"replace","old_text":"a","new_text":"a"}),
            ),
            (
                "a",
                json!({"operation":"insert","text":"","placement":"start"}),
            ),
            (
                "a",
                json!({"operation":"insert","text":"b","placement":"start","anchor":"a"}),
            ),
            (
                "a",
                json!({"operation":"insert","text":"b","placement":"before"}),
            ),
            (
                "a",
                json!({"operation":"insert","text":"b","placement":"before","anchor":""}),
            ),
        ] {
            assert_eq!(
                apply_edits(old, &edits(json!([edit]))),
                Err(RepositoryError::Invalid)
            );
        }
        assert!(
            serde_json::from_value::<TextEdit>(
                json!({"operation":"insert","text":"x","placement":"start","anchor":null})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<TextEdit>(
                json!({"operation":"replace","old_text":"a","new_text":"b","path":"a"})
            )
            .is_err()
        );
        assert_eq!(apply_edits("x", &[]), Err(RepositoryError::Invalid));
        assert_eq!(
            apply_edits(
                "x",
                &vec![
                    TextEdit::Insert {
                        text: "y".to_owned(),
                        placement: Placement::End,
                        anchor: None
                    };
                    257
                ]
            ),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            apply_edits(
                "a",
                &edits(
                    json!([{"operation":"replace","old_text":"a","new_text":"b"},{"operation":"replace","old_text":"b","new_text":"a"}])
                )
            ),
            Err(RepositoryError::Invalid)
        );
        assert_eq!(
            apply_edits(
                "a",
                &edits(
                    json!([{"operation":"insert","text":"x".repeat(MAX_PROJECT_FILE_BYTES),"placement":"end"}])
                )
            ),
            Err(RepositoryError::Invalid)
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn resources_discover_canonical_files_and_reject_aliases() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![ProjectFile {
                    path: "dir/a #%.py".to_owned(),
                    content: "alpha\nbeta".to_owned(),
                }],
                "dir/a #%.py".to_owned(),
                vec![],
                "",
            )
            .await
            .unwrap();
        let server = server(&repository);
        let resources = rpc(&server, "resources/list", json!({})).await;
        assert_eq!(
            resources["result"]["resources"][0]["uri"],
            "faktory://models"
        );
        let templates = rpc(&server, "resources/templates/list", json!({})).await;
        assert_eq!(
            templates["result"]["resourceTemplates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|template| template["uriTemplate"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "faktory://models/{model_id}",
                "faktory://models/{model_id}/project",
                "faktory://models/{model_id}/open",
                "faktory://models/{model_id}/files",
                "faktory://models/{model_id}/files/{path}",
                "faktory://models/{model_id}/revisions/{revision}/files/{path}",
                "faktory://models/{model_id}/hints",
                "faktory://models/{model_id}/releases",
                "faktory://models/{model_id}/releases/{version}",
                "faktory://models/{model_id}/views",
                "faktory://models/{model_id}/views/{view_id}",
                "faktory://models/{model_id}/views/{view_id}/image",
                "faktory://models/{model_id}/images/{output_id}/{style}/{projection}",
            ]
        );
        assert_eq!(
            rpc(&server, "resources/list", json!({"cursor":"unknown"})).await["error"]["code"],
            -32602
        );
        assert_eq!(
            rpc(
                &server,
                "resources/templates/list",
                json!({"cursor":"unknown"})
            )
            .await["error"]["code"],
            -32602
        );
        let collection = rpc(&server, "resources/read", json!({"uri":"faktory://models"})).await;
        let value: Value = serde_json::from_str(
            collection["result"]["contents"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["models"][0]["uri"], "faktory://models/part");
        let files = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/part/files"}),
        )
        .await;
        let files: Value =
            serde_json::from_str(files["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        let uri = format!(
            "faktory://models/part/revisions/{}/files/dir%2Fa%20%23%25.py",
            model.desired_source_revision
        );
        assert_eq!(files["files"][1]["uri"], uri);
        let read = rpc(&server, "resources/read", json!({"uri":uri})).await;
        assert_eq!(read["result"]["contents"][0]["mimeType"], "text/plain");
        assert_eq!(read["result"]["contents"][0]["text"], "alpha\nbeta");
        assert_eq!(
            read["result"]["_meta"]["revision"],
            model.desired_source_revision
        );
        for uri in [
            "file:///etc/passwd",
            "faktory://models/part/files/../x",
            "faktory://models/part/files/%41GENTS.md",
            "faktory://models/part/files/AGENTS.md?x",
            "faktory://models/part/files/%2Fetc",
            "faktory://models/part/unknown",
            "faktory://models/missing",
        ] {
            assert_eq!(
                rpc(&server, "resources/read", json!({"uri":uri})).await["error"]["code"],
                -32602
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn guarded_edits_are_atomic_protect_generated_text_and_support_empty_files_and_hints() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "main.py".to_owned(),
                        content: "alpha".to_owned(),
                    },
                    ProjectFile {
                        path: "empty.py".to_owned(),
                        content: String::new(),
                    },
                ],
                "main.py".to_owned(),
                vec![],
                "",
            )
            .await
            .unwrap();
        let server = server(&repository);
        let input = |uri: &str, revision: &str, edits: Value| {
            serde_json::from_value::<TextEditInput>(
                json!({"uri":uri,"expected_revision":revision,"edits":edits}),
            )
            .unwrap()
        };
        let operations = json!([{"operation":"replace","old_text":"alpha","new_text":"beta"},{"operation":"replace","old_text":"missing","new_text":"x"}]);
        assert_eq!(
            server
                .edit_text(input(
                    "faktory://models/part/files/main.py",
                    &created.desired_source_revision,
                    operations
                ))
                .await
                .unwrap_err(),
            RepositoryError::Invalid
        );
        assert_eq!(
            repository
                .get_model("part")
                .await
                .unwrap()
                .record
                .desired_source_revision,
            created.desired_source_revision
        );
        for uri in [
            "faktory://models/part/files/AGENTS.md",
            &format!(
                "faktory://models/part/revisions/{}/files/main.py",
                created.desired_source_revision
            ),
        ] {
            assert_eq!(
                server
                    .edit_text(input(
                        uri,
                        &created.desired_source_revision,
                        json!([{"operation":"insert","text":"x","placement":"end"}])
                    ))
                    .await
                    .unwrap_err(),
                RepositoryError::Invalid
            );
        }
        let updated = server
            .edit_text(input(
                "faktory://models/part/files/empty.py",
                &created.desired_source_revision,
                json!([{"operation":"insert","text":"\u{feff}body\r\n","placement":"start"}]),
            ))
            .await
            .unwrap();
        assert_ne!(
            updated.desired_source_revision,
            created.desired_source_revision
        );
        let project = repository
            .get_project("part", &updated.desired_source_revision)
            .await
            .unwrap();
        assert_eq!(
            project
                .caller_files()
                .iter()
                .find(|file| file.path == "empty.py")
                .unwrap()
                .content,
            "body\n"
        );
        assert_eq!(
            server
                .edit_text(input(
                    "faktory://models/part/hints",
                    &created.desired_source_revision,
                    json!([{"operation":"insert","text":"Keep parametric.","placement":"start"}])
                ))
                .await
                .unwrap_err(),
            RepositoryError::Conflict
        );
        let updated = server
            .edit_text(input(
                "faktory://models/part/hints",
                &updated.desired_source_revision,
                json!([{"operation":"insert","text":"Keep parametric.","placement":"start"}]),
            ))
            .await
            .unwrap();
        let project = repository
            .get_project("part", &updated.desired_source_revision)
            .await
            .unwrap();
        assert_eq!(project.hints().unwrap(), "Keep parametric.");
        assert!(
            project
                .agents_md()
                .unwrap()
                .contains("# Dependency Guidance")
        );
        assert_eq!(
            server
                .edit_text(input(
                    "faktory://models/part/hints",
                    &updated.desired_source_revision,
                    json!([{"operation":"insert","text":"\n# Spoof","placement":"end"}])
                ))
                .await
                .unwrap_err(),
            RepositoryError::Invalid
        );
    }

    #[tokio::test]
    async fn stored_images_are_binary_resources_and_query_preserves_semantic_images() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("part", "Part", b"alpha")
            .await
            .unwrap();
        repository
            .complete_render(
                "part",
                &model.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .unwrap();
        let view = repository
            .put_view(
                "part",
                named_view(String::new(), faktory_proto::v1::Projection::Perspective),
                None,
            )
            .await
            .unwrap();
        let server = server(&repository);
        let image_uri = "faktory://models/part/images/primary/technical/front";
        let read = rpc(&server, "resources/read", json!({"uri":image_uri})).await;
        assert_eq!(read["result"]["contents"][0]["mimeType"], "image/png");
        let decoded = BASE64
            .decode(read["result"]["contents"][0]["blob"].as_str().unwrap())
            .unwrap();
        validate_projection_png(&decoded).unwrap();
        let view_uri = format!("faktory://models/part/views/{}/image", view.id);
        assert_eq!(
            rpc(&server, "resources/read", json!({"uri":view_uri})).await["error"]["code"],
            -32602
        );
        repository
            .complete_view_render(
                "part",
                &ViewRenderIdentity {
                    revision: model.desired_source_revision.clone(),
                    output_id: "primary".to_owned(),
                    view_id: view.id.clone(),
                    view_etag: view.etag.clone(),
                },
                valid_projection_png(),
            )
            .await
            .unwrap();
        let query = rpc(&server,"tools/call",json!({"name":"query","arguments":{"action":"view.render","input":{"model_id":"part","view_id":view.id}}})).await;
        assert_eq!(query["result"]["content"][1]["type"], "image");
        assert_eq!(query["result"]["content"][1]["mimeType"], "image/png");
        repository
            .edit_project(
                "part",
                &model.desired_source_revision,
                None,
                Some(
                    &ProjectEdit::new(vec![ProjectOperation::FileTextSet {
                        path: "source.py".to_owned(),
                        content: "beta".to_owned(),
                    }])
                    .unwrap(),
                ),
            )
            .await
            .unwrap();
        let stale = rpc(&server, "resources/read", json!({"uri":image_uri})).await;
        let metadata: Value =
            serde_json::from_str(stale["result"]["contents"][1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["stale"], true);
        assert_eq!(metadata["rendered_revision"], model.desired_source_revision);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn release_resources_link_exact_immutable_source_and_errors_are_safe() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let model = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "main.py".to_owned(),
                        content: "result = 1".to_owned(),
                    },
                    ProjectFile {
                        path: "faktory_model/__init__.py".to_owned(),
                        content: "value = 1".to_owned(),
                    },
                ],
                "main.py".to_owned(),
                vec![],
                "",
            )
            .await
            .unwrap();
        repository
            .complete_render(
                "part",
                &model.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .unwrap();
        repository
            .publish_model_release(
                "part",
                Version::new(1, 0, 0),
                &model.desired_source_revision,
            )
            .await
            .unwrap();
        let server = server(&repository);
        let release = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/part/releases/1.0.0"}),
        )
        .await;
        let release: Value =
            serde_json::from_str(release["result"]["contents"][0]["text"].as_str().unwrap())
                .unwrap();
        let file = release["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "faktory_model/__init__.py")
            .unwrap();
        assert!(file.get("content").is_none());
        let read = rpc(&server, "resources/read", json!({"uri":file["uri"]})).await;
        assert_eq!(read["result"]["contents"][0]["text"], "value = 1");
        assert_eq!(
            read["result"]["_meta"]["revision"],
            model.desired_source_revision
        );
        store
            .put(
                &crate::model::project::project_key("part", &model.desired_source_revision),
                bytes::Bytes::from_static(b"private corrupt source"),
                PutCondition::Any,
            )
            .await
            .unwrap();
        let read = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/part/project"}),
        )
        .await;
        assert_eq!(read["error"]["code"], -32603);
        assert!(!read.to_string().contains("private corrupt source"));
        let key = crate::model::project::project_key("part", &model.desired_source_revision);
        let etag = store.get(&key).await.unwrap().etag;
        store.delete(&key, &etag).await.unwrap();
        assert_eq!(
            rpc(
                &server,
                "resources/read",
                json!({"uri":"faktory://models/part/project"})
            )
            .await["error"]["code"],
            -32603
        );
        assert_eq!(rpc(&server,"resources/read", json!({"uri":format!("faktory://models/part/revisions/{}/files/main.py", "0".repeat(64))})).await["error"]["code"],-32602);
    }

    #[tokio::test]
    async fn view_creation_and_guarded_update_are_distinct_actions() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        repository
            .create_model("part", "Part", b"alpha")
            .await
            .unwrap();
        let server = server(&repository);
        let mut view = json!({"name":"Front","target":[0,0,0],"rotation":[0,0,0,1],"projection":"PERSPECTIVE","distance":10,"field_of_view_degrees":45,"orthographic_scale":10});
        let create = rpc(&server,"tools/call",json!({"name":"create","arguments":{"action":"view.create","input":{"model_id":"part","view":view}}})).await;
        let saved = &create["result"]["structuredContent"]["view"];
        let id = saved["id"].as_str().unwrap();
        assert!(!id.is_empty());
        view["id"] = json!(id);
        let create = rpc(&server,"tools/call",json!({"name":"create","arguments":{"action":"view.create","input":{"model_id":"part","view":view}}})).await;
        assert_eq!(create["error"]["code"], -32602);
        let create = rpc(&server,"tools/call",json!({"name":"create","arguments":{"action":"view.create","input":{"model_id":"part","view":view,"expected_etag":null}}})).await;
        assert_eq!(create["error"]["code"], -32602);
        view["name"] = json!("Updated");
        let update = rpc(&server,"tools/call",json!({"name":"execute","arguments":{"action":"view.update","input":{"model_id":"part","view":view,"expected_etag":saved["etag"]}}})).await;
        assert_eq!(
            update["result"]["structuredContent"]["view"]["name"],
            "Updated"
        );
        let update = rpc(&server,"tools/call",json!({"name":"execute","arguments":{"action":"view.update","input":{"model_id":"part","view":view,"expected_etag":saved["etag"]}}})).await;
        assert_eq!(update["result"]["isError"], true);
        view["id"] = json!("missing");
        let update = rpc(&server,"tools/call",json!({"name":"execute","arguments":{"action":"view.update","input":{"model_id":"part","view":view,"expected_etag":"stale"}}})).await;
        assert_eq!(update["result"]["isError"], true);
        assert_eq!(repository.list_views("part").await.unwrap().len(), 1);
        for action in ["model.destroy", "model.release.destroy"] {
            assert_eq!(
                rpc(
                    &server,
                    "tools/call",
                    json!({"name":"destroy","arguments":{"action":action,"input":{"model_id":"part"}}})
                )
                .await["error"]["code"],
                -32602
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::literal_string_with_formatting_args)]
    async fn simple_template_expansion_round_trips_whole_nested_reserved_paths() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let path = "nested/a#b:c%.py";
        repository
            .create_project_from_files(
                "part",
                "Part",
                vec![ProjectFile {
                    path: path.to_owned(),
                    content: "body".to_owned(),
                }],
                path.to_owned(),
                vec![],
                "",
            )
            .await
            .unwrap();
        let server = server(&repository);
        let templates = rpc(&server, "resources/templates/list", json!({})).await;
        let template = templates["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|template| template["uriTemplate"] == "faktory://models/{model_id}/files/{path}")
            .unwrap();
        // RFC 6570 simple expansion escapes everything except ALPHA / DIGIT / - . _ ~.
        // Use the independent URL form encoder with its three form-specific differences removed.
        let expanded_path = url::form_urlencoded::byte_serialize(path.as_bytes())
            .collect::<String>()
            .replace('+', "%20")
            .replace("%7E", "~")
            .replace('*', "%2A");
        assert_eq!(expanded_path, "nested%2Fa%23b%3Ac%25.py");
        let uri = template["uriTemplate"]
            .as_str()
            .unwrap()
            .replace("{model_id}", "part")
            .replace("{path}", &expanded_path);
        let read = rpc(&server, "resources/read", json!({"uri":uri})).await;
        assert_eq!(read["result"]["contents"][0]["text"], "body");
        assert_eq!(read["result"]["_meta"]["path"], path);
        let files = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/part/files"}),
        )
        .await;
        let files: Value =
            serde_json::from_str(files["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(files["files"][1]["edit_uri"], uri);
        for alias in [
            "faktory://models/part/files/nested/a%23b%3Ac%25.py",
            "faktory://models/part/files/nested%2fa%23b%3Ac%25.py",
            "faktory://models/part/files/nested%2Fa#b:c%.py",
        ] {
            assert_eq!(
                rpc(&server, "resources/read", json!({"uri":alias})).await["error"]["code"],
                -32602
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn historical_locked_projects_keep_bytes_hashes_readiness_resources_and_release_closure()
    {
        use crate::model::{
            model_key,
            project::{ProjectBundle, project_key},
        };
        // Generated from the exact render_agents literal and canonical field order at
        // 4c5cf0f435f28b97928927a1a84b18b103e9c25b, independently of the current generator.
        let bytes = include_bytes!("legacy-locked-project.json");
        let revision = "251b2f2c5d53eccbb7b4a83b63f6639aac0fc5bf4eff353c6a65808215f9e007";
        let historical: ProjectBundle = serde_json::from_slice(bytes).unwrap();
        assert_eq!(historical.digest().unwrap(), revision);
        assert_eq!(historical.canonical_bytes().unwrap(), bytes);
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let dep = repository
            .create_project_from_files(
                "dep",
                "Dependency",
                historical.caller_files(),
                "main.py".to_owned(),
                vec![],
                "",
            )
            .await
            .unwrap();
        assert_eq!(
            dep.desired_source_revision,
            historical.locks[0].project_revision
        );
        repository
            .complete_render(
                "dep",
                &dep.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .unwrap();
        let release = repository
            .publish_model_release("dep", Version::new(1, 0, 0), &dep.desired_source_revision)
            .await
            .unwrap();
        assert_eq!(release.digest, historical.locks[0].release_sha256);
        let mut model = repository
            .create_project_from_files(
                "legacy",
                "Legacy",
                historical.caller_files(),
                "main.py".to_owned(),
                historical.requirements.clone(),
                "Legacy hints",
            )
            .await
            .unwrap();
        model.desired_source_revision = revision.to_owned();
        let key = project_key("legacy", revision);
        store
            .put(&key, Bytes::from_static(bytes), PutCondition::Absent)
            .await
            .unwrap();
        store
            .put(
                &model_key("legacy"),
                Bytes::from(serde_json::to_vec(&model).unwrap()),
                PutCondition::Any,
            )
            .await
            .unwrap();
        let original = store.get(&key).await.unwrap();
        repository.ready().await.unwrap();
        assert_eq!(
            repository.get_project("legacy", revision).await.unwrap(),
            historical
        );
        repository
            .complete_render("legacy", revision, rendered_output(valid_projection_png()))
            .await
            .unwrap();
        repository
            .publish_model_release("legacy", Version::new(1, 0, 0), revision)
            .await
            .unwrap();
        let server = server(&repository);
        let uri = format!("faktory://models/legacy/revisions/{revision}/files/AGENTS.md");
        assert_historical_noops_do_not_commit_or_submit(
            &repository,
            store.as_ref(),
            &server,
            &historical,
        )
        .await;
        let read = rpc(&server, "resources/read", json!({"uri":uri})).await;
        assert_eq!(
            read["result"]["contents"][0]["text"],
            historical.agents_md().unwrap()
        );
        let read = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/legacy/project"}),
        )
        .await;
        let read: Value =
            serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(read["revision"], revision);
        repository
            .validate_project_closure("legacy", historical.clone())
            .await
            .unwrap();
        for replacement in [
            "spoofed guidance",
            &historical
                .agents_md()
                .unwrap()
                .replace("Legacy hints\n", "Legacy hints\r\n"),
            &historical
                .agents_md()
                .unwrap()
                .replace("# Index\n", "# Index\n\n"),
            &historical.agents_md().unwrap().replace(
                "Inspect exact files with MCP model.release.get(model_id=\"dep\", version=\"1.0.0\").",
                "Inspect exact files with MCP resources/read URI faktory://models/dep/releases/1.0.0.",
            ),
        ] {
            let mut forged = historical.clone();
            forged.files[0].content = replacement.to_owned();
            let digest = forged.digest().unwrap();
            store
                .put(
                    &project_key("legacy", &digest),
                    Bytes::from(forged.canonical_bytes().unwrap()),
                    PutCondition::Absent,
                )
                .await
                .unwrap();
            assert_eq!(
                repository.get_project("legacy", &digest).await.unwrap_err(),
                RepositoryError::Corrupt
            );
        }
        let edited = repository
            .edit_project(
                "legacy",
                revision,
                None,
                Some(
                    &ProjectEdit::new(vec![ProjectOperation::FileTextSet {
                        path: "main.py".to_owned(),
                        content: "result = 2".to_owned(),
                    }])
                    .unwrap(),
                ),
            )
            .await
            .unwrap()
            .record;
        assert_ne!(edited.desired_source_revision, revision);
        let new = repository
            .get_project("legacy", &edited.desired_source_revision)
            .await
            .unwrap();
        assert!(
            new.agents_md()
                .unwrap()
                .contains("MCP resources/read URI faktory://models/dep/releases/1.0.0")
        );
        assert!(!new.agents_md().unwrap().contains("model.read("));
        repository.ready().await.unwrap();
        let read = rpc(
            &server,
            "resources/read",
            json!({"uri":"faktory://models/legacy/releases/1.0.0"}),
        )
        .await;
        let read: Value =
            serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(read["release"]["project_revision"], revision);
        assert_eq!(read["closure"].as_array().unwrap().len(), 2);
        let retained = store.get(&key).await.unwrap();
        assert_eq!(retained.bytes, original.bytes);
        assert_eq!(retained.etag, original.etag);
        assert_eq!(
            repository
                .get_project("legacy", revision)
                .await
                .unwrap()
                .digest()
                .unwrap(),
            revision
        );
    }

    #[tokio::test]
    async fn create_never_upserts_and_dispatch_rejects_old_tools_and_wrong_actions() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("part", "Part", b"alpha")
            .await
            .unwrap();
        let server = server(&repository);
        for arguments in [
            json!({"action":"model.glob","model_id":"part","pattern":"*"}),
            json!({"action":"model.glob","input":null}),
            json!({"action":"model.glob","input":{"model_id":"part","pattern":"*"},"filter":".id"}),
            json!({"action":"model.glob","input":{"model_id":"part","pattern":"*","unknown":true}}),
        ] {
            assert_eq!(
                rpc(
                    &server,
                    "tools/call",
                    json!({"name":"query","arguments":arguments})
                )
                .await["error"]["code"],
                -32602
            );
        }
        let call = rpc(&server,"tools/call",json!({"name":"create","arguments":{"action":"file.create","input":{"model_id":"part","expected_revision":model.desired_source_revision,"path":"source.py","content":"overwrite"}}})).await;
        assert_eq!(call["result"]["isError"], true);
        let project = repository
            .get_project("part", &model.desired_source_revision)
            .await
            .unwrap();
        assert_eq!(project.caller_files()[0].content, "alpha");
        let call = rpc(
            &server,
            "tools/call",
            json!({"name":"query","arguments":{"action":"model.get","input":{"model_id":"part"}}}),
        )
        .await;
        assert_eq!(call["error"]["code"], -32602);
        let call = rpc(
            &server,
            "tools/call",
            json!({"name":"model.read","arguments":{"model_id":"part","path":"source.py"}}),
        )
        .await;
        assert!(call.get("error").is_some());
        for input in [
            json!({"uri":"x","expected_revision":"x","edits":[],"name":"x"}),
            json!({"uri":"x","expected_revision":"x","edits":[{"operation":"file.delete","path":"x"}]}),
        ] {
            assert!(serde_json::from_value::<TextEditInput>(input).is_err());
        }
    }
}
