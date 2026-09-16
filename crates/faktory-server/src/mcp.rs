//! MCP tools over the same repository and render queue.

mod workspace;

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mcp::{
    McpProtectedResourceMetadata, McpToolCall, McpToolDefinition, McpToolResult,
    OAuthAuthorizationServer,
    server::{
        ServerContext, ServerError, ServerResult, StreamableHttpAuthorization,
        StreamableHttpOptions, streamable_http_router_with_options,
    },
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    model::{
        ModelRecord, Repository, RepositoryError, TechnicalProjection, ViewRenderIdentity,
        project::{
            DirectRequirement, ProjectEdit, ProjectFile, ProjectOperation, package_namespace,
        },
        release::ModelRelease,
        rollout::{ModelReleaseRolloutRecord, RolloutModelState},
    },
    render::{
        PROJECTION_HEIGHT, PROJECTION_WIDTH, RenderQueue, validate_projection_png,
        validate_visual_png,
    },
    visual::{VISUAL_RECIPE, VisualCoordinator, VisualRenderSpec, VisualRendererError},
};

#[derive(Clone, Debug)]
pub struct FaktoryMcp {
    repository: Repository,
    renders: RenderQueue,
    visual: VisualCoordinator,
}

impl FaktoryMcp {
    #[must_use]
    pub fn new(repository: Repository, renders: RenderQueue) -> Self {
        let visual = renders.visual();
        Self {
            repository,
            renders,
            visual,
        }
    }

    pub fn router(self) -> axum::Router {
        self.streamable_http_router()
    }

    pub fn hosted_router(
        self,
        resource: String,
        issuer: String,
        oauth: OAuthAuthorizationServer,
    ) -> Result<axum::Router, String> {
        let scopes = vec!["faktory:use".to_owned()];
        let metadata = McpProtectedResourceMetadata::new(resource, [issuer])
            .with_scopes(scopes.clone())
            .with_resource_name("Faktory MCP");
        let authorization = StreamableHttpAuthorization::hosted(metadata, move |token, context| {
            oauth.authorize_token(token, context)
        })
        .map_err(|_| "invalid MCP authorization configuration".to_owned())?
        .with_required_scopes(scopes);
        let options = StreamableHttpOptions::default()
            .without_root_protected_resource_metadata()
            .with_authorization(authorization);
        Ok(streamable_http_router_with_options(Arc::new(self), options))
    }
}

#[mcp::mcp_server(
    name = "faktory",
    version = "0.1.0",
    description = "Faktory model and shared-view tools.",
    auth(
        scopes = ["faktory:use"],
        required_scopes = ["faktory:use"],
        realm = "faktory",
        resource_name = "Faktory MCP"
    )
)]
impl FaktoryMcp {
    #[tool(name = "model.list", definition = model_list_definition())]
    async fn model_list(&self, _: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        match self.repository.list_models().await {
            Ok(models) => Ok(result(json!({ "models": models }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.get", definition = model_get_definition())]
    async fn model_get(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelIdInput = parse(call)?;
        match get_model_with_project(&self.repository, &input.model_id).await {
            Ok((model, project)) => Ok(result(json!({ "model": model, "project": project }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.open", definition = model_open_definition())]
    async fn model_open(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelIdInput = parse(call)?;
        match workspace::model_open(&self.repository, &input.model_id).await {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.read", definition = model_read_definition())]
    async fn model_read(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelReadInput = parse(call)?;
        match workspace::model_read(&self.repository, &input).await {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.glob", definition = model_glob_definition())]
    async fn model_glob(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelGlobInput = parse(call)?;
        match workspace::model_glob(
            &self.repository,
            &input.model_id,
            &input.pattern,
            input.revision.as_deref(),
        )
        .await
        {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.grep", definition = model_grep_definition())]
    async fn model_grep(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelGrepInput = parse(call)?;
        match workspace::model_grep(&self.repository, &input).await {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.inspect", definition = model_inspect_definition())]
    async fn model_inspect(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: InspectInput = parse(call)?;
        match inspect_model(
            &self.repository,
            &input.model_id,
            input.output_id.as_deref(),
            input.projection,
            input.render_style,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "view.inspect", definition = view_inspect_definition())]
    async fn view_inspect(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ViewIdInput = parse(call)?;
        match inspect_view(
            &self.repository,
            &self.visual,
            &input.model_id,
            &input.view_id,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.create", definition = model_create_definition())]
    async fn model_create(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: CreateInput = parse(call)?;
        match create_and_schedule(self.repository.clone(), self.renders.clone(), input).await {
            Ok(model) => Ok(result(json!({ "model": model }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.edit", definition = model_edit_definition())]
    async fn model_edit(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: EditInput = parse(call)?;
        match edit_and_schedule(self.repository.clone(), self.renders.clone(), input).await {
            Ok(model) => Ok(result(json!({ "model": model }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.apply_patch", definition = model_apply_patch_definition())]
    async fn model_apply_patch(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ApplyPatchInput = parse(call)?;
        match apply_patch_and_schedule(self.repository.clone(), self.renders.clone(), input).await {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.release.list", definition = model_release_list_definition())]
    async fn model_release_list(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ModelIdInput = parse(call)?;
        match self.repository.list_model_releases(&input.model_id).await {
            Ok(releases) => Ok(result(
                json!({ "model_id": input.model_id, "package": package_namespace(&input.model_id), "releases": releases.iter().map(model_release_value).collect::<Vec<_>>() }),
            )),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.release.get", definition = model_release_get_definition())]
    async fn model_release_get(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ModelReleaseGetInput = parse(call)?;
        match model_release_get_value(&self.repository, &input).await {
            Ok(value) => Ok(result(value)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.release.publish", definition = model_release_publish_definition())]
    async fn model_release_publish(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ModelReleasePublishInput = parse(call)?;
        match publish_and_rollout(self.repository.clone(), self.renders.clone(), input).await {
            Ok(output) => Ok(result(output)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "model.render.retry", definition = model_retry_definition())]
    async fn model_retry(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ModelIdInput = parse(call)?;
        match retry_and_schedule(self.repository.clone(), self.renders.clone(), input).await {
            Ok(model) => Ok(result(json!({ "model": model }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "view.list", definition = view_list_definition())]
    async fn view_list(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: ModelIdInput = parse(call)?;
        match self.repository.list_views(&input.model_id).await {
            Ok(views) => Ok(result(json!({ "views": views }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "view.put", definition = view_put_definition())]
    async fn view_put(&self, call: McpToolCall, _: ServerContext) -> ServerResult<McpToolResult> {
        let input: PutViewInput = parse(call)?;
        match self
            .repository
            .put_view(
                &input.model_id,
                input.view.into_proto(),
                input.expected_etag.as_deref(),
            )
            .await
        {
            Ok(view) => Ok(result(json!({ "view": view }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "view.delete", definition = view_delete_definition())]
    async fn view_delete(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: DeleteViewInput = parse(call)?;
        match self
            .repository
            .delete_view(&input.model_id, &input.view_id, &input.expected_etag)
            .await
        {
            Ok(()) => Ok(result(json!({ "deleted": true }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(name = "view.set-default", definition = view_default_definition())]
    async fn view_default(
        &self,
        call: McpToolCall,
        _: ServerContext,
    ) -> ServerResult<McpToolResult> {
        let input: ViewIdInput = parse(call)?;
        match self
            .repository
            .set_default_view(&input.model_id, &input.view_id)
            .await
        {
            Ok(model) => Ok(result(json!({ "model": model }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

async fn get_model_with_project(
    repository: &Repository,
    model_id: &str,
) -> Result<(ModelRecord, crate::model::project::ProjectBundle), RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let project = match repository
        .get_project(&model.id, &model.desired_source_revision)
        .await
    {
        Ok(project) => project,
        Err(RepositoryError::NotFound) => return Err(RepositoryError::Corrupt),
        Err(error) => return Err(error),
    };
    Ok((model, project))
}

async fn inspect_model(
    repository: &Repository,
    model_id: &str,
    output_id: Option<&str>,
    projection: TechnicalProjection,
    render_style: RenderStyle,
) -> Result<McpToolResult, RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let rendered_revision = model.current_successful_source_revision.clone();
    if rendered_revision.is_empty() {
        return Err(RepositoryError::NotFound);
    }
    let output = match output_id {
        Some(output_id) => model
            .effective_outputs()
            .into_iter()
            .find(|output| output.output_id == output_id)
            .ok_or(RepositoryError::NotFound)?,
        None => model.primary_output()?,
    };
    let image = match render_style {
        RenderStyle::Technical => {
            let image = repository
                .output_projection_image(
                    model_id,
                    &rendered_revision,
                    &output.output_id,
                    projection,
                )
                .await?;
            validate_projection_png(&image).map_err(|_| RepositoryError::Corrupt)?;
            image
        }
        RenderStyle::Shaded => {
            if !output.primary {
                return Err(RepositoryError::Invalid);
            }
            let image = repository
                .shaded_projection_image(model_id, &rendered_revision, projection)
                .await?;
            validate_visual_png(&image).map_err(|_| RepositoryError::Corrupt)?;
            image
        }
    };
    let stale = model.desired_source_revision != rendered_revision;
    let text = match (render_style, stale) {
        (RenderStyle::Technical, true) => format!(
            "Warning: showing the last successful {} projection; the desired revision is not rendered.",
            projection.as_str()
        ),
        (RenderStyle::Technical, false) => {
            format!("{} technical projection.", projection.as_str())
        }
        (RenderStyle::Shaded, true) => format!(
            "Warning: showing the last successful shaded {} projection; the desired revision is not rendered.",
            projection.as_str()
        ),
        (RenderStyle::Shaded, false) => format!("{} shaded projection.", projection.as_str()),
    };
    let mut metadata = json!({
        "model_id": model.id,
        "output_id": output.output_id,
        "output_role": output.role,
        "primary": output.primary,
        "projection": projection,
        "width": PROJECTION_WIDTH,
        "height": PROJECTION_HEIGHT,
        "desired_revision": model.desired_source_revision,
        "rendered_revision": rendered_revision,
        "render_state": model.render_state,
        "mime_type": "image/png",
        "stale": stale
    });
    if matches!(render_style, RenderStyle::Shaded) {
        metadata["style"] = json!(render_style);
        metadata["recipe"] = json!(VISUAL_RECIPE);
    }
    Ok(McpToolResult::new(json!({
        "content": [
            { "type": "text", "text": text },
            { "type": "image", "data": BASE64.encode(image), "mimeType": "image/png" }
        ],
        "structuredContent": { "metadata": metadata },
        "isError": false
    })))
}

#[allow(clippy::too_many_lines)]
async fn inspect_view(
    repository: &Repository,
    visual: &VisualCoordinator,
    model_id: &str,
    view_id: &str,
) -> Result<McpToolResult, RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let revision = model.current_successful_source_revision.clone();
    if revision.is_empty() {
        return Err(RepositoryError::NotFound);
    }
    let view = repository.get_view(model_id, view_id).await?.record;
    let primary = model.primary_output()?;
    let identity = ViewRenderIdentity {
        revision: revision.clone(),
        output_id: primary.output_id.clone(),
        view_id: view.id.clone(),
        view_etag: view.etag.clone(),
    };
    let image = match repository.cached_view_image(model_id, &identity).await {
        Ok(image) => {
            validate_visual_png(&image).map_err(|_| RepositoryError::Corrupt)?;
            image
        }
        Err(RepositoryError::NotFound) => {
            let glb = repository.geometry(model_id, &revision).await?;
            let spec = VisualRenderSpec::from_view(&view).map_err(map_visual_error)?;
            let rendered = visual
                .render_view(identity.clone(), glb, spec)
                .await
                .map_err(map_visual_error)?;
            let [(name, image)] = rendered
                .images
                .try_into()
                .map_err(|_| RepositoryError::Corrupt)?;
            if name != "view" {
                return Err(RepositoryError::Corrupt);
            }
            validate_visual_png(&image).map_err(|_| RepositoryError::Corrupt)?;
            repository
                .complete_view_render(model_id, &identity, image.clone())
                .await?;
            image
        }
        Err(error) => return Err(error),
    };
    let current = repository.get_model(model_id).await?.record;
    if current.current_successful_source_revision != revision {
        return Err(RepositoryError::Conflict);
    }
    let current_view = repository.get_view(model_id, view_id).await?.record;
    if current_view.etag != view.etag {
        return Err(RepositoryError::Conflict);
    }
    let stale = current.desired_source_revision != revision;
    let text = if stale {
        "Warning: showing the saved view from the last successful revision; the desired revision is not rendered."
    } else {
        "Saved shaded view."
    };
    let metadata = json!({
        "model_id": current.id,
        "output_id": primary.output_id,
        "output_role": primary.role,
        "primary": true,
        "view_id": current_view.id,
        "view_etag": current_view.etag,
        "style": "shaded",
        "recipe": VISUAL_RECIPE,
        "width": PROJECTION_WIDTH,
        "height": PROJECTION_HEIGHT,
        "desired_revision": current.desired_source_revision,
        "rendered_revision": revision,
        "render_state": current.render_state,
        "mime_type": "image/png",
        "stale": stale
    });
    Ok(McpToolResult::new(json!({
        "content": [
            { "type": "text", "text": text },
            { "type": "image", "data": BASE64.encode(image), "mimeType": "image/png" }
        ],
        "structuredContent": { "metadata": metadata },
        "isError": false
    })))
}

const fn map_visual_error(error: VisualRendererError) -> RepositoryError {
    match error {
        VisualRendererError::Unavailable => RepositoryError::Unavailable,
        VisualRendererError::InvalidResponse => RepositoryError::Corrupt,
    }
}

async fn create_and_schedule(
    repository: Repository,
    renders: RenderQueue,
    input: CreateInput,
) -> Result<ModelRecord, RepositoryError> {
    let reservation = renders.reserve().await?;
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let model = task_repository
            .create_project_from_files(
                &input.model_id,
                &input.name,
                input.files,
                input.entrypoint,
                input.requirements,
                &input.hints,
            )
            .await?;
        reservation.submit(model.id.clone(), model.desired_source_revision.clone());
        Ok(model)
    });
    join_scheduled(&repository, task).await
}

async fn edit_and_schedule(
    repository: Repository,
    renders: RenderQueue,
    input: EditInput,
) -> Result<ModelRecord, RepositoryError> {
    let project = input.operations.map(ProjectEdit::new).transpose()?;
    if project.is_none() {
        return repository
            .edit_project(
                &input.model_id,
                &input.expected_revision,
                input.name.as_deref(),
                None,
            )
            .await
            .map(|edited| edited.record);
    }
    let reservation = renders.reserve().await?;
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let edited = task_repository
            .edit_project(
                &input.model_id,
                &input.expected_revision,
                input.name.as_deref(),
                project.as_ref(),
            )
            .await?;
        debug_assert!(edited.source_changed);
        reservation.submit(
            edited.record.id.clone(),
            edited.record.desired_source_revision.clone(),
        );
        Ok(edited.record)
    });
    join_scheduled(&repository, task).await
}

async fn apply_patch_and_schedule(
    repository: Repository,
    renders: RenderQueue,
    input: ApplyPatchInput,
) -> Result<Value, RepositoryError> {
    let parsed = workspace::parse_patch(&input.patch)?;
    let project = ProjectEdit::new(parsed.operations)?;
    let previous_revision = input.expected_revision.clone();
    let reservation = renders.reserve().await?;
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let edited = task_repository
            .edit_project(
                &input.model_id,
                &input.expected_revision,
                None,
                Some(&project),
            )
            .await?;
        debug_assert!(edited.source_changed);
        reservation.submit(
            edited.record.id.clone(),
            edited.record.desired_source_revision.clone(),
        );
        Ok(json!({
            "model": edited.record,
            "previous_revision": previous_revision,
            "new_revision": edited.record.desired_source_revision,
            "changed_paths": parsed.changed_paths,
            "renders_scheduled": 1
        }))
    });
    join_scheduled(&repository, task).await
}

async fn publish_and_rollout(
    repository: Repository,
    renders: RenderQueue,
    input: ModelReleasePublishInput,
) -> Result<Value, RepositoryError> {
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let release = task_repository
            .publish_model_release(&input.model_id, input.version, &input.expected_revision)
            .await?;
        let compatible = task_repository
            .list_model_releases(&release.model_id)
            .await?
            .iter()
            .any(|candidate| {
                candidate.version != release.version
                    && candidate.version.major == release.version.major
            });
        let (rollout, scheduled) = if compatible {
            let rollout = task_repository.rollout_model_release(&release).await?;
            let scheduled = renders.reconcile(&task_repository).await?;
            (rollout_value(&rollout), scheduled)
        } else {
            (
                json!({
                    "status": "not_applicable",
                    "complete": true,
                    "counts": {"updated": 0, "already_current": 0, "not_eligible": 0}
                }),
                0,
            )
        };
        Ok(json!({
            "release": {
                "model_id": release.model_id,
                "version": release.version,
                "project_revision": release.project_revision,
                "release_sha256": release.digest
            },
            "rollout": rollout,
            "renders_scheduled": scheduled
        }))
    });
    join_scheduled(&repository, task).await
}

fn model_release_value(release: &ModelRelease) -> Value {
    json!({
        "model_id": release.model_id,
        "package": package_namespace(&release.model_id),
        "version": release.version,
        "project_revision": release.project_revision,
        "release_sha256": release.digest,
    })
}

async fn model_release_get_value(
    repository: &Repository,
    input: &ModelReleaseGetInput,
) -> Result<Value, RepositoryError> {
    let release = repository
        .get_model_release(&input.model_id, &input.version)
        .await?;
    let mut closure = repository
        .resolve_project_closure(&release.model_id, &release.project_revision)
        .await?;
    let root = closure
        .iter_mut()
        .find(|item| item.identity.model_id == release.model_id)
        .ok_or(RepositoryError::Corrupt)?;
    root.identity.version = Some(release.version.clone());
    root.identity.release_sha256 = Some(release.digest.clone());
    let project = repository
        .get_project(&release.model_id, &release.project_revision)
        .await?;
    Ok(
        json!({"release": model_release_value(&release), "closure": closure.iter().map(|item| &item.identity).collect::<Vec<_>>(), "files": workspace::file_index(&project.files)}),
    )
}

fn rollout_value(rollout: &ModelReleaseRolloutRecord) -> Value {
    let mut updated = 0;
    let mut already_current = 0;
    let mut not_eligible = 0;
    for state in rollout.models.values() {
        match state {
            RolloutModelState::Updated => updated += 1,
            RolloutModelState::AlreadyCurrent => already_current += 1,
            RolloutModelState::NotEligible => not_eligible += 1,
        }
    }
    json!({
        "status": if rollout.complete { "complete" } else { "incomplete" },
        "complete": rollout.complete,
        "counts": {
            "updated": updated,
            "already_current": already_current,
            "not_eligible": not_eligible
        }
    })
}

async fn retry_and_schedule(
    repository: Repository,
    renders: RenderQueue,
    input: ModelIdInput,
) -> Result<ModelRecord, RepositoryError> {
    let reservation = renders.reserve().await?;
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let model = task_repository.retry_render(&input.model_id).await?;
        reservation.submit(model.id.clone(), model.desired_source_revision.clone());
        Ok(model)
    });
    join_scheduled(&repository, task).await
}

async fn join_scheduled<T>(
    repository: &Repository,
    task: tokio::task::JoinHandle<Result<T, RepositoryError>>,
) -> Result<T, RepositoryError> {
    task.await.unwrap_or_else(|_| {
        repository.mark_degraded();
        tracing::warn!("render scheduling task failed");
        Err(RepositoryError::Unavailable)
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelIdInput {
    model_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelReadInput {
    model_id: String,
    path: String,
    revision: Option<String>,
    #[serde(default = "default_line_offset")]
    offset: usize,
    #[serde(default = "default_read_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelGlobInput {
    model_id: String,
    pattern: String,
    revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelGrepInput {
    model_id: String,
    pattern: String,
    include: Option<String>,
    revision: Option<String>,
    #[serde(default = "default_grep_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPatchInput {
    model_id: String,
    expected_revision: String,
    patch: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectInput {
    model_id: String,
    output_id: Option<String>,
    projection: TechnicalProjection,
    #[serde(default)]
    render_style: RenderStyle,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RenderStyle {
    #[default]
    Technical,
    Shaded,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    model_id: String,
    name: String,
    files: Vec<ProjectFile>,
    entrypoint: String,
    #[serde(default)]
    requirements: Vec<DirectRequirement>,
    #[serde(default)]
    hints: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    model_id: String,
    expected_revision: String,
    name: Option<String>,
    operations: Option<Vec<ProjectOperation>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelReleaseGetInput {
    model_id: String,
    version: Version,
}

const fn default_line_offset() -> usize {
    1
}

const fn default_read_limit() -> usize {
    workspace::DEFAULT_READ_LIMIT
}

const fn default_grep_limit() -> usize {
    workspace::DEFAULT_GREP_LIMIT
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelReleasePublishInput {
    model_id: String,
    version: Version,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewIdInput {
    model_id: String,
    view_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteViewInput {
    model_id: String,
    view_id: String,
    expected_etag: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutViewInput {
    model_id: String,
    view: ViewInput,
    expected_etag: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewInput {
    #[serde(default)]
    id: String,
    name: String,
    target: [f64; 3],
    rotation: [f64; 4],
    projection: String,
    distance: f64,
    field_of_view_degrees: f64,
    orthographic_scale: f64,
}

impl ViewInput {
    fn into_proto(self) -> faktory_proto::v1::NamedView {
        faktory_proto::v1::NamedView {
            id: self.id,
            name: self.name,
            target: Some(faktory_proto::v1::Vector3 {
                x: self.target[0],
                y: self.target[1],
                z: self.target[2],
            }),
            rotation: Some(faktory_proto::v1::Quaternion {
                x: self.rotation[0],
                y: self.rotation[1],
                z: self.rotation[2],
                w: self.rotation[3],
            }),
            projection: match self.projection.as_str() {
                "PERSPECTIVE" => faktory_proto::v1::Projection::Perspective.into(),
                "ORTHOGRAPHIC" => faktory_proto::v1::Projection::Orthographic.into(),
                _ => faktory_proto::v1::Projection::Unspecified.into(),
            },
            distance: self.distance,
            field_of_view_degrees: self.field_of_view_degrees,
            orthographic_scale: self.orthographic_scale,
            etag: String::new(),
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(call: McpToolCall) -> ServerResult<T> {
    serde_json::from_value(call.arguments)
        .map_err(|_| ServerError::invalid_params("invalid arguments"))
}

fn result(value: Value) -> McpToolResult {
    mcp::progressive::tool_result(value, None)
        .expect("unfiltered JSON output must produce a tool result")
}

fn tool_error(error: RepositoryError) -> McpToolResult {
    let (code, message, retryable) = match error {
        RepositoryError::Invalid => ("invalid_argument", "Arguments are invalid.", false),
        RepositoryError::NotFound => ("not_found", "The requested record was not found.", false),
        RepositoryError::Conflict => ("conflict", "The record changed concurrently.", true),
        RepositoryError::Unavailable => {
            ("unavailable", "Faktory is temporarily unavailable.", true)
        }
        RepositoryError::Corrupt => ("invalid_state", "Stored state is invalid.", false),
    };
    McpToolResult::new(json!({
        "content": [{ "type": "text", "text": message }],
        "structuredContent": { "error": { "code": code, "message": message, "retryable": retryable } },
        "isError": true
    }))
}

fn definition(name: &str, description: &str, read_only: bool, schema: Value) -> McpToolDefinition {
    McpToolDefinition {
        name: name.to_owned(),
        title: None,
        description: Some(description.to_owned()),
        icons: Vec::new(),
        input_schema: schema,
        output_schema: None,
        annotations: Some(json!({
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": read_only,
            "openWorldHint": false
        })),
        meta: None,
    }
}

fn model_list_definition() -> McpToolDefinition {
    definition(
        "model.list",
        "List model metadata.",
        true,
        json!({ "type": "object", "additionalProperties": false }),
    )
}
fn model_get_definition() -> McpToolDefinition {
    definition(
        "model.get",
        "Get model metadata and the complete canonical desired-revision project, including every file (including generated AGENTS.md), the entrypoint, direct dependency requirements, and exact server-controlled locks. This is the hard-cutover multi-file contract; no legacy source field is returned.",
        true,
        model_id_schema(),
    )
}
fn model_open_definition() -> McpToolDefinition {
    definition(
        "model.open",
        "Open the desired model project with model metadata, revision identity, entrypoint, requirements, locks, a content-hash file index, and the full generated AGENTS.md. Other file bodies are omitted.",
        true,
        model_id_schema(),
    )
}
fn model_read_definition() -> McpToolDefinition {
    definition(
        "model.read",
        "Read a bounded line range from one file in the desired or an exact immutable model revision. The response identifies the actual revision used.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "path": project_read_path_schema(),
                "revision": revision_schema("Optional exact immutable revision; defaults to the current desired revision."),
                "offset": line_offset_schema(),
                "limit": read_limit_schema()
            },
            "required": ["model_id", "path"],
            "additionalProperties": false
        }),
    )
}
fn model_glob_definition() -> McpToolDefinition {
    definition(
        "model.glob",
        "Discover at most 1000 matching paths in the desired or an exact immutable model revision using an ASCII glob.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "pattern": {
                    "type": "string", "minLength": 1, "maxLength": workspace::MAX_PATTERN_BYTES,
                    "description": "ASCII glob matched against complete relative POSIX project paths. '*', '?', and character classes do not cross '/'; valid '**' forms recursively match path components."
                },
                "revision": revision_schema("Optional exact immutable revision; defaults to the current desired revision.")
            },
            "required": ["model_id", "pattern"],
            "additionalProperties": false
        }),
    )
}
fn model_grep_definition() -> McpToolDefinition {
    definition(
        "model.grep",
        "Search UTF-8 project lines with a bounded linear-time Rust regular expression in the desired or an exact immutable revision.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "pattern": {
                    "type": "string", "minLength": 1,
                    "description": "Rust regex pattern, limited to 1024 UTF-8 bytes."
                },
                "include": {
                    "type": "string", "minLength": 1, "maxLength": workspace::MAX_PATTERN_BYTES,
                    "description": "Optional ASCII glob restricting searched project paths. '*', '?', and character classes do not cross '/'; valid '**' forms recursively match path components."
                },
                "revision": revision_schema("Optional exact immutable revision; defaults to the current desired revision."),
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": workspace::MAX_GREP_LIMIT,
                    "default": workspace::DEFAULT_GREP_LIMIT,
                    "description": "Maximum matches returned; each matching line is limited to 2000 UTF-8 bytes."
                }
            },
            "required": ["model_id", "pattern"],
            "additionalProperties": false
        }),
    )
}
fn model_inspect_definition() -> McpToolDefinition {
    definition(
        "model.inspect",
        "Return one bounded technical or shaded projection from the current successful render.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "output_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 64,
                    "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$",
                    "description": "Optional output ID; defaults to the primary output. Shaded inspection is primary-only."
                },
                "projection": {
                    "type": "string",
                    "enum": ["isometric", "front", "back", "left", "right", "top", "bottom"],
                    "description": "Canonical technical projection."
                },
                "render_style": {
                    "type": "string",
                    "enum": ["technical", "shaded"],
                    "default": "technical",
                    "description": "Projection rendering style."
                }
            },
            "required": ["model_id", "projection"],
            "additionalProperties": false
        }),
    )
}
fn view_inspect_definition() -> McpToolDefinition {
    definition(
        "view.inspect",
        "Render one exact saved named view from the current successful model revision.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "view_id": {"type": "string", "minLength": 1, "maxLength": 64}
            },
            "required": ["model_id", "view_id"],
            "additionalProperties": false
        }),
    )
}
fn model_retry_definition() -> McpToolDefinition {
    definition(
        "model.render.retry",
        "Retry a failed desired revision.",
        false,
        model_id_schema(),
    )
}
fn view_list_definition() -> McpToolDefinition {
    definition(
        "view.list",
        "List shared named views.",
        true,
        model_id_schema(),
    )
}
fn model_create_definition() -> McpToolDefinition {
    definition(
        "model.create",
        "Create a canonical multi-file model project with a caller-supplied kebab-case ID. This hard cutover accepts no legacy source or dependencies field. Requirements are optional direct-only compatible ranges and resolve to exact server-controlled locks. Faktory generates AGENTS.md; callers provide only optional Hints content.",
        false,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Display name, limited to 200 UTF-8 bytes."
                },
                "files": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 256,
                    "description": "Caller-owned UTF-8 project files in request order. AGENTS.md is reserved and generated by Faktory.",
                    "items": project_file_schema()
                },
                "entrypoint": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 1024,
                    "description": "Relative POSIX path of the existing nonempty Python entrypoint."
                },
                "requirements": {
                    "type": "array",
                    "maxItems": 64,
                    "default": [],
                    "description": "Optional direct-only model requirements. Each stable range must be exactly >=MAJOR.MINOR.PATCH,<NEXT_MAJOR.0.0; Faktory resolves and stores exact locks.",
                    "items": dependency_schema()
                },
                "hints": {
                    "type": "string",
                    "default": "",
                    "description": "Optional model-specific body for generated AGENTS.md # Hints. Top-level headings are forbidden; Index and Dependency Guidance are protected and server-generated."
                }
            },
            "required": ["model_id", "name", "files", "entrypoint"],
            "additionalProperties": false
        }),
    )
}
fn model_edit_definition() -> McpToolDefinition {
    definition(
        "model.edit",
        "Conditionally edit model metadata and/or one caller-ordered canonical project transaction. This hard cutover accepts no source, dependencies, or grouped project shape. Operations execute exactly in array order and may add, patch, delete, or rename caller files, set the entrypoint, replace direct requirements, or exactly patch only the AGENTS.md Hints body. Generated Index and Dependency Guidance and exact locks are protected and server-controlled.",
        false,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "expected_revision": {
                    "type": "string",
                    "pattern": "^[0-9a-f]{64}$",
                    "description": "Exact desired project revision to edit."
                },
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Display name, limited to 200 UTF-8 bytes."
                },
                "operations": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 256,
                    "description": "Caller-ordered project operations executed exactly from first to last.",
                    "items": project_operation_schema()
                }
            },
            "required": ["model_id", "expected_revision"],
            "anyOf": [
                {"required": ["name"]},
                {"required": ["operations"]}
            ],
            "additionalProperties": false
        }),
    )
}

fn model_apply_patch_definition() -> McpToolDefinition {
    definition(
        "model.apply_patch",
        "Conditionally and atomically apply one stripped file patch envelope to caller-owned project files. The complete envelope is parsed first; every hunk must match exactly once in one project transaction, and exactly one render is scheduled on success.",
        false,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "expected_revision": revision_schema("Exact desired project revision guarding the whole transaction."),
                "patch": {
                    "type": "string",
                    "minLength": 1,
                    "description": "UTF-8 stripped patch envelope, limited to 1,048,576 bytes, with Begin/End Patch and ordered Add, Update, optional Move, and Delete sections. AGENTS.md is protected."
                }
            },
            "required": ["model_id", "expected_revision", "patch"],
            "additionalProperties": false
        }),
    )
}

fn model_release_list_definition() -> McpToolDefinition {
    definition(
        "model.release.list",
        "List immutable releases for one model.",
        true,
        json!({"type":"object","properties":{"model_id":model_id_property()},"required":["model_id"],"additionalProperties":false}),
    )
}

fn model_release_get_definition() -> McpToolDefinition {
    definition(
        "model.release.get",
        "Get immutable model-release metadata, namespace, closure identities, and file hashes without source bodies.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "version": stable_version_schema()
            },
            "required": ["model_id", "version"],
            "additionalProperties": false
        }),
    )
}

fn model_release_publish_definition() -> McpToolDefinition {
    definition(
        "model.release.publish",
        "Permanently publish one immutable model release from an exact READY project revision; same-major releases roll compatible consumers.",
        false,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "version": stable_version_schema(),
                "expected_revision": revision_schema("Exact READY desired and current-successful project revision.")
            },
            "required": ["model_id", "version", "expected_revision"],
            "additionalProperties": false
        }),
    )
}

fn project_operation_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "file.add"},
                    "path": project_path_schema(),
                    "content": {"type": "string"}
                },
                "required": ["operation", "path", "content"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "file.patch"},
                    "path": project_path_schema(),
                    "patches": exact_patches_schema()
                },
                "required": ["operation", "path", "patches"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "file.delete"},
                    "path": project_path_schema()
                },
                "required": ["operation", "path"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "file.rename"},
                    "from": project_path_schema(),
                    "to": project_path_schema()
                },
                "required": ["operation", "from", "to"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "entrypoint.set"},
                    "path": project_path_schema()
                },
                "required": ["operation", "path"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "dependencies.set"},
                    "requirements": {
                        "type": "array",
                        "maxItems": 64,
                        "description": "Complete direct-requirement replacement; empty removes all requirements.",
                        "items": dependency_schema()
                    }
                },
                "required": ["operation", "requirements"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "operation": {"const": "hints.patch"},
                    "patches": exact_patches_schema()
                },
                "required": ["operation", "patches"],
                "additionalProperties": false
            }
        ]
    })
}

fn project_path_schema() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 1024})
}

fn project_read_path_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 1024,
        "description": "Strict relative ASCII POSIX file path."
    })
}

fn revision_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[0-9a-f]{64}$",
        "description": description
    })
}

fn line_offset_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": 1,
        "description": "One-based starting line."
    })
}

fn read_limit_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": workspace::MAX_READ_LIMIT,
        "default": workspace::DEFAULT_READ_LIMIT,
        "description": "Maximum lines returned."
    })
}

fn exact_patches_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 256,
        "items": {
            "type": "object",
            "properties": {
                "old": {"type": "string", "minLength": 1},
                "new": {"type": "string"}
            },
            "required": ["old", "new"],
            "additionalProperties": false
        }
    })
}

fn project_file_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "minLength": 1, "maxLength": 1024},
            "content": {"type": "string"}
        },
        "required": ["path", "content"],
        "additionalProperties": false
    })
}

fn dependency_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model_id": model_id_property(),
            "range": {
                "type": "string",
                "pattern": "^>=[0-9]+\\.[0-9]+\\.[0-9]+,<[0-9]+\\.0\\.0$",
                "description": "Exact stable same-major range >=MAJOR.MINOR.PATCH,<NEXT_MAJOR.0.0."
            }
        },
        "required": ["model_id", "range"],
        "additionalProperties": false
    })
}

fn stable_version_schema() -> Value {
    json!({
        "type": "string",
        "pattern": "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$",
        "description": "Exact stable SemVer MAJOR.MINOR.PATCH with no prerelease, build suffix, or leading zero."
    })
}
fn view_put_definition() -> McpToolDefinition {
    definition(
        "view.put",
        "Create or conditionally update a shared view.",
        false,
        json!({ "type":"object", "properties": { "model_id":{"type":"string"}, "view":{"type":"object"}, "expected_etag":{"type":"string"} }, "required":["model_id","view"], "additionalProperties":false }),
    )
}
fn view_delete_definition() -> McpToolDefinition {
    definition(
        "view.delete",
        "Conditionally delete a shared view.",
        false,
        json!({ "type":"object", "properties": { "model_id":{"type":"string"}, "view_id":{"type":"string"}, "expected_etag":{"type":"string"} }, "required":["model_id","view_id","expected_etag"], "additionalProperties":false }),
    )
}
fn view_default_definition() -> McpToolDefinition {
    definition(
        "view.set-default",
        "Select a model's default shared view.",
        false,
        json!({ "type":"object", "properties": { "model_id":{"type":"string"}, "view_id":{"type":"string"} }, "required":["model_id","view_id"], "additionalProperties":false }),
    )
}
fn model_id_schema() -> Value {
    json!({ "type":"object", "properties": { "model_id": model_id_property() }, "required":["model_id"], "additionalProperties":false })
}

fn model_id_property() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 64,
        "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$",
        "description": "Lowercase ASCII kebab-case model ID."
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use bytes::Bytes;
    use tokio::sync::Semaphore;
    use tower::ServiceExt as _;

    use super::*;
    use crate::{
        model::{
            GeometryFactsRecord, GeometrySizeRecord, ModelOutputSummaryRecord, OutputManifest,
            OutputRoleRecord, RenderedModelOutput, RenderedOutput, SourcePatch, StoredRenderState,
            TechnicalProjectionImages,
            project::{ExactPatch, project_key},
        },
        render::RenderConfig,
        storage::{InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject},
        visual::{VisualRenderResult, VisualRenderer},
    };
    use faktory_proto::v1::{NamedView, Projection, Quaternion, Vector3};

    #[derive(Debug)]
    struct BlockingCommittedPut {
        inner: InMemoryObjectStore,
        block_model_put: AtomicBool,
        committed: Semaphore,
        release: Semaphore,
    }

    impl BlockingCommittedPut {
        fn new() -> Self {
            Self {
                inner: InMemoryObjectStore::default(),
                block_model_put: AtomicBool::new(true),
                committed: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl ObjectStore for BlockingCommittedPut {
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
            let result = self.inner.put(key, bytes, condition).await;
            if key.ends_with("/model.json") && self.block_model_put.swap(false, Ordering::SeqCst) {
                self.committed.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .expect("release semaphore open")
                    .forget();
            }
            result
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    #[derive(Debug)]
    struct BlockingFirstProjectGet {
        inner: InMemoryObjectStore,
        block_project_get: AtomicBool,
        project_get_started: Semaphore,
        release_project_get: Semaphore,
    }

    impl BlockingFirstProjectGet {
        fn new() -> Self {
            Self {
                inner: InMemoryObjectStore::default(),
                block_project_get: AtomicBool::new(false),
                project_get_started: Semaphore::new(0),
                release_project_get: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl ObjectStore for BlockingFirstProjectGet {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            if key.ends_with("/project.json")
                && self.block_project_get.swap(false, Ordering::SeqCst)
            {
                self.project_get_started.add_permits(1);
                self.release_project_get
                    .acquire()
                    .await
                    .expect("release semaphore open")
                    .forget();
            }
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
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    fn valid_projection_png() -> Bytes {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
            .expect("projection pixmap");
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
        Bytes::from(pixmap.encode_png().expect("encode projection PNG"))
    }

    fn pseudo_projection_png() -> Bytes {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&PROJECTION_WIDTH.to_be_bytes());
        bytes.extend_from_slice(&PROJECTION_HEIGHT.to_be_bytes());
        Bytes::from(bytes)
    }

    fn rendered_output(image: Bytes) -> RenderedOutput {
        let summary = ModelOutputSummaryRecord {
            output_id: "primary".to_owned(),
            role: OutputRoleRecord::Assembly,
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
                projections: TechnicalProjectionImages::all(image),
                shaded: Some(TechnicalProjectionImages::all(valid_projection_png())),
            }],
        }
    }

    fn named_view(id: String, projection: Projection) -> NamedView {
        NamedView {
            id,
            name: "Saved camera".to_owned(),
            target: Some(Vector3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }),
            rotation: Some(Quaternion {
                x: 0.1,
                y: 0.2,
                z: 0.3,
                w: 0.9,
            }),
            projection: projection.into(),
            distance: 12.0,
            field_of_view_degrees: 55.0,
            orthographic_scale: 7.0,
            etag: String::new(),
        }
    }

    #[derive(Debug)]
    struct RecordingVisualRenderer {
        calls: AtomicUsize,
        specs: Mutex<Vec<VisualRenderSpec>>,
        started: Semaphore,
        release: Semaphore,
        blocking: bool,
    }

    impl RecordingVisualRenderer {
        fn immediate() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                specs: Mutex::new(Vec::new()),
                started: Semaphore::new(0),
                release: Semaphore::new(0),
                blocking: false,
            }
        }

        fn blocking() -> Self {
            Self {
                blocking: true,
                ..Self::immediate()
            }
        }
    }

    #[async_trait]
    impl VisualRenderer for RecordingVisualRenderer {
        async fn render(
            &self,
            _glb: Bytes,
            spec: VisualRenderSpec,
        ) -> Result<VisualRenderResult, VisualRendererError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.specs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec);
            self.started.add_permits(1);
            if self.blocking {
                self.release.acquire().await.expect("release open").forget();
            }
            Ok(VisualRenderResult {
                images: vec![("view".to_owned(), valid_projection_png())],
            })
        }
    }

    #[test]
    fn successful_result_puts_complete_json_in_text_and_structured_content() {
        let output = json!({
            "model": {
                "model_id": "part",
                "desired_source_revision": "abc123"
            }
        });
        let result = result(output.clone());
        let text = result.raw["content"][0]["text"].as_str().unwrap();

        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), output);
        assert_eq!(result.raw["structuredContent"], output);
    }

    #[tokio::test]
    async fn model_get_returns_complete_canonical_project_with_matching_desired_revision() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let files = vec![
            ProjectFile {
                path: "main.py".to_owned(),
                content: "# caf\u{e9}\nfrom helper import box".to_owned(),
            },
            ProjectFile {
                path: "helper.py".to_owned(),
                content: "box = 1".to_owned(),
            },
        ];
        let created = repository
            .create_project_from_files(
                "part",
                "Part",
                files,
                "main.py".to_owned(),
                Vec::new(),
                "Keep dimensions parametric.",
            )
            .await
            .expect("create model");

        let (model, project) = get_model_with_project(&repository, "part")
            .await
            .expect("get model project");
        let tool_result = result(json!({"model": model, "project": project}));
        let output = tool_result.raw["structuredContent"].clone();
        let text = tool_result.raw["content"][0]["text"]
            .as_str()
            .expect("text content");

        assert!(output.get("source").is_none());
        assert_eq!(output["project"]["entrypoint"], "main.py");
        assert_eq!(output["project"]["requirements"], json!([]));
        assert_eq!(output["project"]["locks"], json!([]));
        assert_eq!(output["project"]["files"].as_array().map(Vec::len), Some(3));
        assert!(output["project"]["files"].to_string().contains("AGENTS.md"));
        assert!(
            output["project"]["files"]
                .to_string()
                .contains("Keep dimensions parametric.")
        );
        assert_eq!(
            output["model"]["desired_source_revision"],
            created.desired_source_revision
        );
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), output);
        assert_eq!(tool_result.raw["structuredContent"], output);
        assert!(output.get("storage_etag").is_none());
        assert!(output["model"].get("storage_etag").is_none());
        assert!(output["model"].get("etag").is_none());
    }

    #[tokio::test]
    async fn model_get_returns_desired_project_instead_of_current_successful_project() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"old source")
            .await
            .expect("create model");
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(Bytes::from_static(b"png")),
            )
            .await
            .expect("complete initial render");
        let edited = repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "old".to_owned(),
                    new: "desired".to_owned(),
                }]),
            )
            .await
            .expect("edit model");

        let (model, project) = get_model_with_project(&repository, "part")
            .await
            .expect("get desired project");

        assert_eq!(
            project
                .files
                .iter()
                .find(|file| file.path == "source.py")
                .map(|file| file.content.as_str()),
            Some("desired source")
        );
        assert_eq!(
            model.current_successful_source_revision,
            created.desired_source_revision
        );
        assert_eq!(
            model.desired_source_revision,
            edited.record.desired_source_revision
        );
        assert_ne!(
            model.desired_source_revision,
            model.current_successful_source_revision
        );
    }

    #[tokio::test]
    async fn model_inspect_returns_one_semantic_png_with_fresh_metadata() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let image = valid_projection_png();
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(image.clone()),
            )
            .await
            .expect("complete render");
        let output = inspect_model(
            &repository,
            "part",
            None,
            TechnicalProjection::Front,
            RenderStyle::Technical,
        )
        .await
        .expect("inspect result")
        .raw;

        assert_eq!(output["isError"], false);
        assert_eq!(output["content"].as_array().unwrap().len(), 2);
        assert_eq!(output["content"][0]["type"], "text");
        assert_eq!(output["content"][1]["type"], "image");
        assert_eq!(output["content"][1]["mimeType"], "image/png");
        let decoded = BASE64
            .decode(output["content"][1]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, image);
        validate_projection_png(&decoded).expect("semantic image is a valid projection PNG");
        let metadata = &output["structuredContent"]["metadata"];
        assert_eq!(metadata["model_id"], "part");
        assert_eq!(metadata["projection"], "front");
        assert_eq!(metadata["width"], 640);
        assert_eq!(metadata["height"], 480);
        assert_eq!(
            metadata["desired_revision"],
            created.desired_source_revision
        );
        assert_eq!(
            metadata["rendered_revision"],
            created.desired_source_revision
        );
        assert_eq!(metadata["render_state"], "READY");
        assert_eq!(metadata["mime_type"], "image/png");
        assert_eq!(metadata["stale"], false);
        assert!(
            !metadata
                .to_string()
                .contains(output["content"][1]["data"].as_str().unwrap())
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn streamable_http_tools_call_preserves_semantic_image_block() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .complete_render(
                &model.id,
                &model.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete render");
        let view = repository
            .put_view(
                "part",
                named_view(String::new(), Projection::Perspective),
                None,
            )
            .await
            .expect("create saved view");
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
            .expect("cache saved view");
        let renders = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["unused".to_owned()],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 1024,
            },
        )
        .expect("render queue");
        let router = FaktoryMcp::new(repository, renders).router();
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "model.inspect")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "model.inspect",
                        "arguments": {"model_id": "part", "projection": "right"},
                        "_meta": {
                            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                            "io.modelcontextprotocol/clientCapabilities": {},
                            "io.modelcontextprotocol/clientInfo": {
                                "name": "faktory-test",
                                "version": "1.0.0"
                            }
                        }
                    }
                })
                .to_string(),
            ))
            .expect("MCP request");

        let response = router.clone().oneshot(request).await.expect("MCP response");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        assert_eq!(status, StatusCode::OK, "MCP response: {body:?}");
        let body = std::str::from_utf8(&body).expect("UTF-8 response");
        let payload = body
            .strip_prefix("data: ")
            .and_then(|body| body.strip_suffix("\n\n"))
            .unwrap_or(body);
        let response: Value = serde_json::from_str(payload).expect("JSON-RPC response");
        let content = response["result"]["content"]
            .as_array()
            .expect("semantic content");
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
        let image = BASE64
            .decode(content[1]["data"].as_str().expect("image data"))
            .expect("base64 image");
        validate_projection_png(&image).expect("transported projection PNG");

        let discovery = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/list")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{},
                    "io.modelcontextprotocol/clientInfo":{"name":"faktory-test","version":"1.0.0"}
                }}})
                .to_string(),
            ))
            .expect("discovery request");
        let discovery = router
            .clone()
            .oneshot(discovery)
            .await
            .expect("discovery response");
        let body = to_bytes(discovery.into_body(), 1024 * 1024)
            .await
            .expect("discovery body");
        assert!(
            std::str::from_utf8(&body)
                .expect("UTF-8 discovery")
                .contains("view.inspect"),
            "discovery response: {body:?}"
        );

        let view_call = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "view.inspect")
            .body(Body::from(
                json!({
                    "jsonrpc":"2.0", "id":3, "method":"tools/call",
                    "params":{"name":"view.inspect","arguments":{"model_id":"part","view_id":view.id},"_meta":{
                        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities":{},
                        "io.modelcontextprotocol/clientInfo":{"name":"faktory-test","version":"1.0.0"}
                    }}
                })
                .to_string(),
            ))
            .expect("view call request");
        let response = router.oneshot(view_call).await.expect("view call response");
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("view call body");
        let body = std::str::from_utf8(&body).expect("UTF-8 view response");
        assert!(body.contains("\"style\":\"shaded\""));
        assert!(body.contains("\"type\":\"image\""));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn shaded_model_inspect_returns_stored_semantic_image_and_legacy_is_safe() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let image = valid_projection_png();
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(image.clone()),
            )
            .await
            .expect("complete render");

        let output = inspect_model(
            &repository,
            "part",
            None,
            TechnicalProjection::Top,
            RenderStyle::Shaded,
        )
        .await
        .expect("shaded inspect");
        assert_eq!(output.raw["content"][1]["type"], "image");
        assert_eq!(
            output.raw["structuredContent"]["metadata"]["style"],
            "shaded"
        );
        assert_eq!(
            output.raw["structuredContent"]["metadata"]["recipe"],
            VISUAL_RECIPE
        );
        repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "source".to_owned(),
                    new: "changed".to_owned(),
                }]),
            )
            .await
            .expect("make shaded render stale");
        let stale = inspect_model(
            &repository,
            "part",
            None,
            TechnicalProjection::Top,
            RenderStyle::Shaded,
        )
        .await
        .expect("stale shaded inspect");
        assert_eq!(stale.raw["structuredContent"]["metadata"]["stale"], true);

        let key = crate::model::output_shaded_projection_key(
            "part",
            &created.desired_source_revision,
            "primary",
            TechnicalProjection::Top,
        );
        let stored = store.get(&key).await.expect("stored shaded image");
        store
            .delete(&key, &stored.etag)
            .await
            .expect("remove legacy image");
        assert_eq!(
            inspect_model(
                &repository,
                "part",
                None,
                TechnicalProjection::Top,
                RenderStyle::Shaded,
            )
            .await
            .expect_err("legacy render lacks shaded image"),
            RepositoryError::NotFound
        );

        let corrupt_repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let corrupt = corrupt_repository
            .create_model("corrupt", "Corrupt", b"source")
            .await
            .expect("create corrupt model");
        let mut output = rendered_output(valid_projection_png());
        output.outputs[0].shaded = Some(TechnicalProjectionImages::all(pseudo_projection_png()));
        corrupt_repository
            .complete_render(&corrupt.id, &corrupt.desired_source_revision, output)
            .await
            .expect("store corrupt shaded fixture");
        assert_eq!(
            inspect_model(
                &corrupt_repository,
                "corrupt",
                None,
                TechnicalProjection::Top,
                RenderStyle::Shaded,
            )
            .await
            .expect_err("corrupt shaded projection rejected"),
            RepositoryError::Corrupt
        );
    }

    #[tokio::test]
    async fn view_inspect_converts_exact_camera_then_caches_the_result() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete render");
        let view = repository
            .put_view(
                "part",
                named_view(String::new(), Projection::Orthographic),
                None,
            )
            .await
            .expect("create view");
        let renderer = Arc::new(RecordingVisualRenderer::immediate());
        let visual = VisualCoordinator::new(renderer.clone(), Duration::from_secs(1));
        let persisted_view = repository
            .get_view("part", &view.id)
            .await
            .expect("load persisted view")
            .record;

        for _ in 0..2 {
            let output = inspect_view(&repository, &visual, "part", &view.id)
                .await
                .expect("inspect view");
            let metadata = &output.raw["structuredContent"]["metadata"];
            assert_eq!(metadata["view_id"], view.id);
            assert_eq!(metadata["view_etag"], view.etag);
            assert_eq!(metadata["style"], "shaded");
            assert_eq!(metadata["recipe"], VISUAL_RECIPE);
            assert_eq!(metadata["stale"], false);
            assert_eq!(output.raw["content"][1]["mimeType"], "image/png");
        }
        assert_eq!(renderer.calls.load(Ordering::SeqCst), 1);
        let specs = renderer
            .specs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            specs.as_slice(),
            &[VisualRenderSpec::View {
                recipe: VISUAL_RECIPE,
                camera: crate::visual::VisualCamera {
                    target: [1.0, 2.0, 3.0],
                    rotation: persisted_view.rotation,
                    projection: crate::visual::VisualProjection::Orthographic,
                    distance: 12.0,
                    field_of_view_degrees: 55.0,
                    orthographic_scale: 7.0,
                },
            }]
        );
        drop(specs);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn view_inspect_rejects_view_races_and_corrupt_cache() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete render");
        let view = repository
            .put_view(
                "part",
                named_view(String::new(), Projection::Perspective),
                None,
            )
            .await
            .expect("create view");
        let renderer = Arc::new(RecordingVisualRenderer::blocking());
        let visual = VisualCoordinator::new(renderer.clone(), Duration::from_secs(1));
        let call_repository = repository.clone();
        let call_visual = visual.clone();
        let view_id = view.id.clone();
        let call = tokio::spawn(async move {
            inspect_view(&call_repository, &call_visual, "part", &view_id).await
        });
        renderer
            .started
            .acquire()
            .await
            .expect("render started")
            .forget();
        let mut changed = named_view(view.id.clone(), Projection::Perspective);
        changed.name = "Changed".to_owned();
        let changed = repository
            .put_view("part", changed, Some(&view.etag))
            .await
            .expect("change view");
        renderer.release.add_permits(1);
        assert_eq!(
            call.await
                .expect("inspection task")
                .expect_err("etag race conflicts"),
            RepositoryError::Conflict
        );

        repository
            .complete_view_render(
                "part",
                &ViewRenderIdentity {
                    revision: created.desired_source_revision.clone(),
                    output_id: "primary".to_owned(),
                    view_id: changed.id.clone(),
                    view_etag: changed.etag.clone(),
                },
                pseudo_projection_png(),
            )
            .await
            .expect("store corrupt cache fixture");
        assert_eq!(
            inspect_view(&repository, &visual, "part", &changed.id)
                .await
                .expect_err("corrupt cache rejected"),
            RepositoryError::Corrupt
        );

        let deleted_view = repository
            .put_view(
                "part",
                named_view(String::new(), Projection::Perspective),
                None,
            )
            .await
            .expect("create view for deletion race");
        let call_repository = repository.clone();
        let call_visual = visual.clone();
        let deleted_view_id = deleted_view.id.clone();
        let deleted_call = tokio::spawn(async move {
            inspect_view(&call_repository, &call_visual, "part", &deleted_view_id).await
        });
        renderer
            .started
            .acquire()
            .await
            .expect("deletion render started")
            .forget();
        repository
            .delete_view("part", &deleted_view.id, &deleted_view.etag)
            .await
            .expect("delete view during render");
        renderer.release.add_permits(1);
        assert_eq!(
            deleted_call
                .await
                .expect("deletion inspection task")
                .expect_err("deleted view rejected"),
            RepositoryError::NotFound
        );

        let revision_view = repository
            .put_view(
                "part",
                named_view(String::new(), Projection::Perspective),
                None,
            )
            .await
            .expect("create view for revision race");
        let call_repository = repository.clone();
        let call_visual = visual.clone();
        let revision_view_id = revision_view.id.clone();
        let revision_call = tokio::spawn(async move {
            inspect_view(&call_repository, &call_visual, "part", &revision_view_id).await
        });
        renderer
            .started
            .acquire()
            .await
            .expect("revision render started")
            .forget();
        let edited = repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "source".to_owned(),
                    new: "replacement".to_owned(),
                }]),
            )
            .await
            .expect("edit source during view render")
            .record;
        repository
            .complete_render(
                "part",
                &edited.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete replacement render");
        renderer.release.add_permits(1);
        assert_eq!(
            revision_call
                .await
                .expect("revision inspection task")
                .expect_err("revision race rejected"),
            RepositoryError::Conflict
        );
    }

    #[tokio::test]
    async fn model_inspect_returns_last_good_with_warning_when_stale() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create model");
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete render");
        let edited = repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("edit model")
            .record;

        let output = inspect_model(
            &repository,
            "part",
            None,
            TechnicalProjection::Top,
            RenderStyle::Technical,
        )
        .await
        .expect("last-good inspect")
        .raw;
        assert!(
            output["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Warning:")
        );
        assert_eq!(output["structuredContent"]["metadata"]["stale"], true);
        assert_eq!(
            output["structuredContent"]["metadata"]["desired_revision"],
            edited.desired_source_revision
        );
        assert_eq!(
            output["structuredContent"]["metadata"]["rendered_revision"],
            created.desired_source_revision
        );
        assert_eq!(
            output["structuredContent"]["metadata"]["render_state"],
            "PENDING"
        );
    }

    #[tokio::test]
    async fn model_inspect_safely_errors_without_a_successful_image() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");

        let error = inspect_model(
            &repository,
            "part",
            None,
            TechnicalProjection::Bottom,
            RenderStyle::Technical,
        )
        .await
        .expect_err("no successful render");
        let output = tool_error(error).raw;
        assert_eq!(output["isError"], true);
        assert_eq!(output["structuredContent"]["error"]["code"], "not_found");
        assert_eq!(output["content"].as_array().unwrap().len(), 1);

        let store = Arc::new(InMemoryObjectStore::default());
        let legacy_repository = Repository::new(store.clone(), 1);
        let rendered = legacy_repository
            .create_model("legacy", "Legacy", b"source")
            .await
            .expect("create legacy model");
        legacy_repository
            .complete_render(
                &rendered.id,
                &rendered.desired_source_revision,
                rendered_output(valid_projection_png()),
            )
            .await
            .expect("complete legacy fixture");
        let key = crate::model::output_projection_key(
            &rendered.id,
            &rendered.desired_source_revision,
            "primary",
            TechnicalProjection::Bottom,
        );
        let image = store.get(&key).await.expect("stored image");
        store.delete(&key, &image.etag).await.expect("remove image");
        let missing = inspect_model(
            &legacy_repository,
            "legacy",
            None,
            TechnicalProjection::Bottom,
            RenderStyle::Technical,
        )
        .await
        .expect_err("legacy image absent");
        assert_eq!(
            tool_error(missing).raw["structuredContent"]["error"]["code"],
            "not_found"
        );
    }

    #[tokio::test]
    async fn model_inspect_selects_secondary_technical_output_and_rejects_shaded() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("bundle", "Bundle", b"source")
            .await
            .expect("create model");
        let mut rendered = rendered_output(valid_projection_png());
        let secondary = RenderedModelOutput {
            summary: ModelOutputSummaryRecord {
                output_id: "secondary".to_owned(),
                role: OutputRoleRecord::Part,
                primary: false,
                facts: GeometryFactsRecord {
                    volume_cubic_millimeters: 1.0,
                    size_millimeters: GeometrySizeRecord {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    },
                },
            },
            glb: Bytes::from_static(b"secondary-glb"),
            preview: Bytes::from_static(b"<svg></svg>"),
            projections: TechnicalProjectionImages::all(valid_projection_png()),
            shaded: None,
        };
        rendered.outputs.push(secondary);
        rendered.manifest = OutputManifest {
            format: OutputManifest::FORMAT.to_owned(),
            outputs: rendered
                .outputs
                .iter()
                .map(|output| output.summary.clone())
                .collect(),
        }
        .canonical_bytes()
        .expect("manifest");
        repository
            .complete_render(&model.id, &model.desired_source_revision, rendered)
            .await
            .expect("complete bundle");

        let inspected = inspect_model(
            &repository,
            &model.id,
            Some("secondary"),
            TechnicalProjection::Top,
            RenderStyle::Technical,
        )
        .await
        .expect("inspect secondary");
        assert_eq!(
            inspected.raw["structuredContent"]["metadata"]["output_id"],
            "secondary"
        );
        assert_eq!(
            inspected.raw["structuredContent"]["metadata"]["output_role"],
            "part"
        );
        assert_eq!(
            inspected.raw["structuredContent"]["metadata"]["primary"],
            false
        );

        assert_eq!(
            inspect_model(
                &repository,
                &model.id,
                Some("secondary"),
                TechnicalProjection::Top,
                RenderStyle::Shaded,
            )
            .await
            .expect_err("secondary shaded inspection is forbidden"),
            RepositoryError::Invalid
        );
    }

    #[tokio::test]
    async fn model_inspect_maps_corrupt_projection_to_safe_invalid_state() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("corrupt", "Corrupt", b"source")
            .await
            .expect("create corrupt model");
        repository
            .complete_render(
                &model.id,
                &model.desired_source_revision,
                rendered_output(pseudo_projection_png()),
            )
            .await
            .expect("complete corrupt fixture");

        let error = inspect_model(
            &repository,
            "corrupt",
            None,
            TechnicalProjection::Isometric,
            RenderStyle::Technical,
        )
        .await
        .expect_err("corrupt image rejected");
        let output = tool_error(error).raw;
        assert_eq!(output["isError"], true);
        assert_eq!(
            output["structuredContent"]["error"]["code"],
            "invalid_state"
        );
        assert_eq!(output["content"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn model_get_pairs_loaded_metadata_with_its_immutable_project_during_edit() {
        let store = Arc::new(BlockingFirstProjectGet::new());
        let repository = Repository::new(store.clone(), 1);
        let created = repository
            .create_model("part", "Part", b"old source")
            .await
            .expect("create model");
        store.block_project_get.store(true, Ordering::SeqCst);

        let get_repository = repository.clone();
        let get =
            tokio::spawn(async move { get_model_with_project(&get_repository, "part").await });
        store
            .project_get_started
            .acquire()
            .await
            .expect("project-start semaphore open")
            .forget();

        let edited = repository
            .edit_model(
                "part",
                &created.desired_source_revision,
                None,
                Some(&[SourcePatch {
                    old: "old".to_owned(),
                    new: "new".to_owned(),
                }]),
            )
            .await
            .expect("edit model");
        store.release_project_get.add_permits(1);

        let (model, project) = get
            .await
            .expect("model get task")
            .expect("get model project");
        assert_eq!(
            model.desired_source_revision,
            created.desired_source_revision
        );
        assert_eq!(
            project
                .files
                .iter()
                .find(|file| file.path == "source.py")
                .map(|file| file.content.as_str()),
            Some("old source")
        );
        assert_ne!(
            model.desired_source_revision,
            edited.record.desired_source_revision
        );
    }

    #[tokio::test]
    async fn model_list_serialization_remains_metadata_only() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        repository
            .create_model("part", "Part", b"private source")
            .await
            .expect("create model");

        let output = json!({
            "models": repository.list_models().await.expect("list models")
        });

        assert_eq!(output["models"].as_array().map(Vec::len), Some(1));
        assert!(output.get("source").is_none());
        assert!(output["models"][0].get("source").is_none());
        assert!(output["models"][0].get("storage_etag").is_none());
        assert!(!output.to_string().contains("private source"));
    }

    #[tokio::test]
    async fn model_get_maps_corrupt_project_to_safe_invalid_state() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let created = repository
            .create_model("part", "Part", b"valid source")
            .await
            .expect("create model");
        store
            .put(
                &project_key("part", &created.desired_source_revision),
                Bytes::from_static(b"\xff\xfe"),
                PutCondition::Any,
            )
            .await
            .expect("corrupt project");

        let error = get_model_with_project(&repository, "part")
            .await
            .expect_err("invalid project must fail");
        assert_eq!(error, RepositoryError::Corrupt);

        let output = tool_error(error);
        assert_eq!(
            output.raw["structuredContent"]["error"]["code"],
            "invalid_state"
        );
        assert_eq!(output.raw["content"][0]["text"], "Stored state is invalid.");
        assert_eq!(output.raw["isError"], true);
    }

    #[tokio::test]
    async fn model_get_distinguishes_missing_model_from_missing_desired_project() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);

        let absent = get_model_with_project(&repository, "absent")
            .await
            .expect_err("absent model must fail");
        assert_eq!(absent, RepositoryError::NotFound);
        assert_eq!(
            tool_error(absent).raw["structuredContent"]["error"]["code"],
            "not_found"
        );

        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let key = project_key("part", &created.desired_source_revision);
        let project = store.get(&key).await.expect("stored project");
        store
            .delete(&key, &project.etag)
            .await
            .expect("delete desired project");

        let missing_project = get_model_with_project(&repository, "part")
            .await
            .expect_err("missing desired project must fail");
        assert_eq!(missing_project, RepositoryError::Corrupt);
        let output = tool_error(missing_project);
        assert_eq!(
            output.raw["structuredContent"]["error"]["code"],
            "invalid_state"
        );
        assert_eq!(output.raw["content"][0]["text"], "Stored state is invalid.");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn create_and_edit_schemas_describe_slug_and_exact_patch_contracts() {
        let create = model_create_definition();
        assert_eq!(create.name, "model.create");
        assert_eq!(
            create.input_schema["properties"]["model_id"]["pattern"],
            "^[a-z0-9]+(-[a-z0-9]+)*$"
        );
        assert_eq!(
            create.input_schema["required"],
            json!(["model_id", "name", "files", "entrypoint"])
        );

        let edit = model_edit_definition();
        assert_eq!(edit.name, "model.edit");
        assert!(create.input_schema["properties"].get("source").is_none());
        assert_eq!(create.input_schema["properties"]["files"]["minItems"], 1);
        assert!(
            create.input_schema["properties"]
                .get("dependencies")
                .is_none()
        );
        assert_eq!(
            create.input_schema["properties"]["requirements"]["default"],
            json!([])
        );
        assert_eq!(create.input_schema["properties"]["hints"]["default"], "");
        assert_eq!(
            create.input_schema["properties"]["files"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(edit.input_schema["properties"]["operations"]["minItems"], 1);
        assert_eq!(
            edit.input_schema["properties"]["operations"]["maxItems"],
            256
        );
        let variants = edit.input_schema["properties"]["operations"]["items"]["oneOf"]
            .as_array()
            .expect("operation variants");
        assert_eq!(variants.len(), 7);
        assert_eq!(
            variants
                .iter()
                .map(|variant| variant["properties"]["operation"]["const"]
                    .as_str()
                    .expect("operation discriminator"))
                .collect::<Vec<_>>(),
            vec![
                "file.add",
                "file.patch",
                "file.delete",
                "file.rename",
                "entrypoint.set",
                "dependencies.set",
                "hints.patch"
            ]
        );
        assert!(
            variants
                .iter()
                .all(|variant| variant["additionalProperties"] == false)
        );
        assert_eq!(
            variants[1]["properties"]["patches"]["items"]["properties"]["old"]["minLength"],
            1
        );
        assert_eq!(
            edit.input_schema["properties"]["expected_revision"]["pattern"],
            "^[0-9a-f]{64}$"
        );
        assert_eq!(edit.input_schema["anyOf"].as_array().map(Vec::len), Some(2));

        let inspect = model_inspect_definition();
        assert_eq!(inspect.name, "model.inspect");
        assert_eq!(
            inspect.input_schema["properties"]["projection"]["enum"],
            json!([
                "isometric",
                "front",
                "back",
                "left",
                "right",
                "top",
                "bottom"
            ])
        );
        assert_eq!(
            inspect.input_schema["required"],
            json!(["model_id", "projection"])
        );
        assert_eq!(
            inspect.input_schema["properties"]["output_id"]["pattern"],
            "^[a-z0-9]+(-[a-z0-9]+)*$"
        );
        assert_eq!(inspect.input_schema["additionalProperties"], false);

        for definition in [
            model_open_definition(),
            model_read_definition(),
            model_glob_definition(),
            model_grep_definition(),
            model_release_list_definition(),
            model_release_get_definition(),
        ] {
            assert_eq!(definition.input_schema["additionalProperties"], false);
            assert_eq!(
                definition.annotations.as_ref().unwrap()["readOnlyHint"],
                true
            );
        }
        let read = model_read_definition();
        assert_eq!(read.input_schema["properties"]["offset"]["default"], 1);
        assert_eq!(
            read.input_schema["properties"]["limit"]["maximum"],
            workspace::MAX_READ_LIMIT
        );
        let grep = model_grep_definition();
        assert_eq!(
            grep.input_schema["properties"]["limit"]["maximum"],
            workspace::MAX_GREP_LIMIT
        );
        assert_eq!(grep.input_schema["properties"]["pattern"]["minLength"], 1);
        assert!(
            grep.input_schema["properties"]["pattern"]
                .get("maxLength")
                .is_none()
        );
        assert_eq!(
            model_glob_definition().input_schema["properties"]["pattern"]["maxLength"],
            workspace::MAX_PATTERN_BYTES
        );
        let patch = model_apply_patch_definition();
        assert_eq!(patch.input_schema["additionalProperties"], false);
        assert_eq!(patch.annotations.as_ref().unwrap()["readOnlyHint"], false);
        assert_eq!(patch.input_schema["properties"]["patch"]["minLength"], 1);
        assert!(
            patch.input_schema["properties"]["patch"]
                .get("maxLength")
                .is_none()
        );

        for definition in [
            model_release_list_definition(),
            model_release_get_definition(),
            model_release_publish_definition(),
        ] {
            assert_eq!(definition.input_schema["additionalProperties"], false);
        }
        let publish = model_release_publish_definition();
        assert_eq!(
            publish.input_schema["properties"]["version"]["pattern"],
            "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$"
        );
        assert_eq!(
            publish.input_schema["properties"]["expected_revision"]["pattern"],
            "^[0-9a-f]{64}$"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn project_and_release_inputs_reject_unknown_nested_fields() {
        assert!(
            serde_json::from_value::<ModelReadInput>(json!({
                "model_id": "part", "path": "main.py", "extra": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ApplyPatchInput>(json!({
                "model_id": "part", "expected_revision": "0".repeat(64),
                "patch": "*** Begin Patch\n*** End Patch", "extra": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ModelGrepInput>(json!({
                "model_id": "part", "pattern": "é".repeat(513)
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ApplyPatchInput>(json!({
                "model_id": "part", "expected_revision": "0".repeat(64),
                "patch": "é".repeat((workspace::MAX_PATCH_BYTES / 2) + 1)
            }))
            .is_ok()
        );
        let minimal = serde_json::from_value::<CreateInput>(json!({
            "model_id": "part",
            "name": "Part",
            "files": [{"path": "main.py", "content": "part = 1"}],
            "entrypoint": "main.py"
        }))
        .expect("requirements and hints are optional");
        assert!(minimal.requirements.is_empty());
        assert!(minimal.hints.is_empty());
        assert!(
            serde_json::from_value::<CreateInput>(json!({
                "model_id": "part",
                "name": "Part",
                "files": [{"path": "main.py", "content": "part = 1"}],
                "entrypoint": "main.py",
                "requirements": [{"model_id": "gears", "range": ">=1.0.0,<2.0.0"}]
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<CreateInput>(json!({
                "model_id": "part",
                "name": "Part",
                "files": [{"path": "main.py", "content": "part = 1", "secret": true}],
                "entrypoint": "main.py"
            }))
            .is_err()
        );
        for obsolete in [json!({"source": "part = 1"}), json!({"dependencies": []})] {
            let mut input = json!({
                "model_id": "part",
                "name": "Part",
                "files": [{"path": "main.py", "content": "part = 1"}],
                "entrypoint": "main.py"
            });
            input
                .as_object_mut()
                .expect("object")
                .extend(obsolete.as_object().expect("obsolete fields").clone());
            assert!(serde_json::from_value::<CreateInput>(input).is_err());
        }
        for obsolete in [
            json!({"source": "part = 2"}),
            json!({"dependencies": []}),
            json!({"project": {"patches": []}}),
        ] {
            let mut input = json!({
                "model_id": "part",
                "expected_revision": "0".repeat(64),
                "name": "Part"
            });
            input
                .as_object_mut()
                .expect("object")
                .extend(obsolete.as_object().expect("obsolete fields").clone());
            assert!(serde_json::from_value::<EditInput>(input).is_err());
        }
        assert!(
            serde_json::from_value::<EditInput>(json!({
                "model_id": "part",
                "expected_revision": "0".repeat(64),
                "operations": [{
                    "operation": "file.patch",
                    "path": "main.py",
                    "patches": [{"old": "1", "new": "2", "extra": false}]
                }]
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ModelReleasePublishInput>(json!({
                "model_id": "gears",
                "version": "1.0.0",
                "expected_revision": "0".repeat(64),
                "extra": 1
            }))
            .is_err()
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn project_edit_is_transactional_and_protects_generated_agents_sections() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "main.py".to_owned(),
                        content: "from helper import value\npart = value".to_owned(),
                    },
                    ProjectFile {
                        path: "helper.py".to_owned(),
                        content: "value = 1".to_owned(),
                    },
                ],
                "main.py".to_owned(),
                vec![],
                "Original hint",
            )
            .await
            .expect("create project");
        let edit_input = serde_json::from_value::<EditInput>(json!({
            "model_id": "part",
            "expected_revision": created.desired_source_revision.clone(),
            "operations": [
                {"operation": "file.add", "path": "dimensions.py", "content": "width = 2"},
                {
                    "operation": "file.patch",
                    "path": "dimensions.py",
                    "patches": [{"old": "width = 2", "new": "width = 3"}]
                },
                {"operation": "file.rename", "from": "helper.py", "to": "helpers/core.py"},
                {
                    "operation": "file.patch",
                    "path": "helpers/core.py",
                    "patches": [{"old": "value = 1", "new": "value = 4"}]
                },
                {
                    "operation": "file.patch",
                    "path": "main.py",
                    "patches": [{"old": "from helper", "new": "from helpers.core"}]
                },
                {
                    "operation": "hints.patch",
                    "patches": [{"old": "Original hint", "new": "Updated hint"}]
                }
            ]
        }))
        .expect("deserialize ordered MCP edit");
        let edit = ProjectEdit::new(edit_input.operations.expect("project operations"))
            .expect("validate ordered MCP edit");
        let edited = repository
            .edit_project("part", &created.desired_source_revision, None, Some(&edit))
            .await
            .expect("edit project");
        let project = repository
            .get_project("part", &edited.record.desired_source_revision)
            .await
            .expect("edited project");
        assert!(
            project
                .files
                .iter()
                .any(|file| file.path == "dimensions.py")
        );
        assert!(
            project
                .files
                .iter()
                .any(|file| file.path == "helpers/core.py")
        );
        assert!(!project.files.iter().any(|file| file.path == "helper.py"));
        assert_eq!(
            project
                .files
                .iter()
                .find(|file| file.path == "dimensions.py")
                .map(|file| file.content.as_str()),
            Some("width = 3")
        );
        assert_eq!(
            project
                .files
                .iter()
                .find(|file| file.path == "helpers/core.py")
                .map(|file| file.content.as_str()),
            Some("value = 4")
        );
        assert!(
            project
                .agents_md()
                .expect("AGENTS")
                .contains("Updated hint")
        );
        assert!(
            project
                .agents_md()
                .expect("AGENTS")
                .starts_with("# Index\n")
        );
        assert!(project.locks.is_empty());

        let protected = ProjectEdit::new(vec![ProjectOperation::FilePatch {
            path: "AGENTS.md".to_owned(),
            patches: vec![ExactPatch {
                old: "Index".to_owned(),
                new: "Owned".to_owned(),
            }],
        }])
        .expect("map protected edit");
        assert!(matches!(
            repository
                .edit_project(
                    "part",
                    &edited.record.desired_source_revision,
                    None,
                    Some(&protected)
                )
                .await,
            Err(RepositoryError::Invalid)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn model_apply_patch_is_atomic_ordered_and_schedules_one_render() {
        let directory = tempfile::tempdir().expect("temp directory");
        let marker = directory.path().join("renders");
        let script = format!("printf x >> '{}'; exit 1", marker.display());
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "main.py".to_owned(),
                        content: "from old import value\nprint(value)".to_owned(),
                    },
                    ProjectFile {
                        path: "old.py".to_owned(),
                        content: "value = 1".to_owned(),
                    },
                    ProjectFile {
                        path: "trash.txt".to_owned(),
                        content: "remove me".to_owned(),
                    },
                ],
                "main.py".to_owned(),
                Vec::new(),
                "",
            )
            .await
            .expect("create project");
        let queue = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 12,
            },
        )
        .expect("render queue");
        let output = apply_patch_and_schedule(
            repository.clone(),
            queue.clone(),
            ApplyPatchInput {
                model_id: "part".to_owned(),
                expected_revision: created.desired_source_revision.clone(),
                patch: "*** Begin Patch\n*** Add File: empty.txt\n*** Update File: old.py\n@@ value\n-value = 1\n+value = 2\n*** Move to: lib/value.py\n*** Update File: main.py\n@@ import\n-from old import value\n+from lib.value import value\n*** Delete File: trash.txt\n*** End Patch"
                    .to_owned(),
            },
        )
        .await
        .expect("apply patch");
        assert_eq!(output["previous_revision"], created.desired_source_revision);
        assert_eq!(output["renders_scheduled"], 1);
        assert_eq!(
            output["changed_paths"],
            json!([
                "empty.txt",
                "old.py",
                "lib/value.py",
                "main.py",
                "trash.txt"
            ])
        );
        let revision = output["new_revision"].as_str().unwrap();
        let project = repository
            .get_project("part", revision)
            .await
            .expect("patched project");
        let file = |path: &str| {
            project
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| file.content.as_str())
        };
        assert_eq!(file("empty.txt"), Some(""));
        assert_eq!(file("lib/value.py"), Some("value = 2"));
        assert_eq!(
            file("main.py"),
            Some("from lib.value import value\nprint(value)")
        );
        assert_eq!(file("old.py"), None);
        assert_eq!(file("trash.txt"), None);

        for _ in 0..100 {
            if tokio::fs::read(&marker)
                .await
                .is_ok_and(|bytes| bytes == b"x")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(tokio::fs::read(&marker).await.unwrap(), b"x");

        let held_reservation = queue.reserve().await.expect("hold queue capacity");
        let malformed = tokio::time::timeout(
            Duration::from_millis(100),
            apply_patch_and_schedule(
                repository.clone(),
                queue.clone(),
                ApplyPatchInput {
                    model_id: "part".to_owned(),
                    expected_revision: revision.to_owned(),
                    patch: "*** Begin Patch\n*** Update File: main.py\n@@ \t\n-old\n+new\n*** End Patch"
                        .to_owned(),
                },
            ),
        )
        .await
        .expect("malformed hunk rejected before queue reservation");
        assert_eq!(malformed.err(), Some(RepositoryError::Invalid));
        drop(held_reservation);

        for (expected_revision, patch) in [
            (
                revision.to_owned(),
                "*** Begin Patch\n*** Add File: staged.txt\n+staged\n*** Update File: main.py\n@@ bad\n-missing\n+replacement\n*** End Patch",
            ),
            (
                created.desired_source_revision,
                "*** Begin Patch\n*** Add File: stale.txt\n+stale\n*** End Patch",
            ),
            (
                revision.to_owned(),
                "*** Begin Patch\n*** Add File: transient.txt\n+value\n*** Delete File: transient.txt\n*** End Patch",
            ),
        ] {
            assert!(
                apply_patch_and_schedule(
                    repository.clone(),
                    queue.clone(),
                    ApplyPatchInput {
                        model_id: "part".to_owned(),
                        expected_revision,
                        patch: patch.to_owned(),
                    },
                )
                .await
                .is_err()
            );
            assert_eq!(
                repository
                    .get_model("part")
                    .await
                    .expect("model")
                    .record
                    .desired_source_revision,
                revision
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(tokio::fs::read(marker).await.unwrap(), b"x");
    }

    #[tokio::test]
    async fn name_only_edit_does_not_wait_for_render_capacity() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let queue = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["unused".to_owned()],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 12,
            },
        )
        .expect("render queue");
        let held = queue.reserve().await.expect("hold render capacity");

        let edited = tokio::time::timeout(
            Duration::from_millis(100),
            edit_and_schedule(
                repository,
                queue,
                EditInput {
                    model_id: "part".to_owned(),
                    expected_revision: created.desired_source_revision,
                    name: Some("Renamed".to_owned()),
                    operations: None,
                },
            ),
        )
        .await
        .expect("name-only edit must not wait for capacity")
        .expect("name-only edit");
        drop(held);

        assert_eq!(edited.name, "Renamed");
        assert_eq!(edited.render_state, StoredRenderState::Pending);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_after_metadata_commit_still_submits_reserved_render() {
        let directory = tempfile::tempdir().expect("temp directory");
        let marker = directory.path().join("rendered");
        let script = format!("printf rendered > '{}'; exit 1", marker.display());
        let store = Arc::new(BlockingCommittedPut::new());
        let repository = Repository::new(store.clone(), 1);
        let queue = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 12,
            },
        )
        .expect("render queue");
        let caller = tokio::spawn(create_and_schedule(
            repository.clone(),
            queue,
            CreateInput {
                model_id: "part".to_owned(),
                name: "Part".to_owned(),
                files: vec![ProjectFile {
                    path: "main.py".to_owned(),
                    content: "source".to_owned(),
                }],
                entrypoint: "main.py".to_owned(),
                requirements: Vec::new(),
                hints: String::new(),
            },
        ));
        store
            .committed
            .acquire()
            .await
            .expect("commit semaphore open")
            .forget();

        caller.abort();
        assert!(caller.await.expect_err("caller canceled").is_cancelled());
        store.release.add_permits(1);

        for _ in 0..100 {
            let models = repository.list_models().await.expect("models");
            if models.len() == 1 && models[0].render_state == StoredRenderState::Failed {
                assert_eq!(
                    tokio::fs::read(&marker).await.expect("renderer marker"),
                    b"rendered"
                );
                repository.ready().await.expect("repository healthy");
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("cancellation prevented committed work from rendering");
    }
}
