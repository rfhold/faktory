//! MCP tools over the same repository and render queue.

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
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::{ModelRecord, Repository, RepositoryError, SourcePatch, TechnicalProjection},
    render::{PROJECTION_HEIGHT, PROJECTION_WIDTH, RenderQueue, validate_projection_png},
};

#[derive(Clone, Debug)]
pub struct FaktoryMcp {
    repository: Repository,
    renders: RenderQueue,
}

impl FaktoryMcp {
    #[must_use]
    pub const fn new(repository: Repository, renders: RenderQueue) -> Self {
        Self {
            repository,
            renders,
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
        match get_model_with_source(&self.repository, &input.model_id).await {
            Ok((model, source)) => Ok(model_get_result(model, source)),
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
        match inspect_model(&self.repository, &input.model_id, input.projection).await {
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

async fn get_model_with_source(
    repository: &Repository,
    model_id: &str,
) -> Result<(ModelRecord, String), RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let source = match repository
        .source(&model.id, &model.desired_source_revision)
        .await
    {
        Ok(source) => source,
        Err(RepositoryError::NotFound) => return Err(RepositoryError::Corrupt),
        Err(error) => return Err(error),
    };
    let source = String::from_utf8(source.to_vec()).map_err(|_| RepositoryError::Corrupt)?;
    Ok((model, source))
}

fn model_get_result(model: ModelRecord, source: String) -> McpToolResult {
    result(json!({ "model": model, "source": source }))
}

async fn inspect_model(
    repository: &Repository,
    model_id: &str,
    projection: TechnicalProjection,
) -> Result<McpToolResult, RepositoryError> {
    let model = repository.get_model(model_id).await?.record;
    let rendered_revision = model.current_successful_source_revision.clone();
    if rendered_revision.is_empty() {
        return Err(RepositoryError::NotFound);
    }
    let image = repository
        .projection_image(model_id, &rendered_revision, projection)
        .await?;
    validate_projection_png(&image).map_err(|_| RepositoryError::Corrupt)?;
    let stale = model.desired_source_revision != rendered_revision;
    let text = if stale {
        format!(
            "Warning: showing the last successful {} projection; the desired revision is not rendered.",
            projection.as_str()
        )
    } else {
        format!("{} technical projection.", projection.as_str())
    };
    let metadata = json!({
        "model_id": model.id,
        "projection": projection,
        "width": PROJECTION_WIDTH,
        "height": PROJECTION_HEIGHT,
        "desired_revision": model.desired_source_revision,
        "rendered_revision": rendered_revision,
        "render_state": model.render_state,
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

async fn create_and_schedule(
    repository: Repository,
    renders: RenderQueue,
    input: CreateInput,
) -> Result<ModelRecord, RepositoryError> {
    let reservation = renders.reserve().await?;
    let task_repository = repository.clone();
    let task = tokio::spawn(async move {
        let model = task_repository
            .create_model(&input.model_id, &input.name, input.source.as_bytes())
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
    if input.patches.is_none() {
        return repository
            .edit_model(
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
        let patches = input.patches.as_deref();
        let edited = task_repository
            .edit_model(
                &input.model_id,
                &input.expected_revision,
                input.name.as_deref(),
                patches,
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
struct InspectInput {
    model_id: String,
    projection: TechnicalProjection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    model_id: String,
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    model_id: String,
    expected_revision: String,
    name: Option<String>,
    patches: Option<Vec<SourcePatch>>,
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
            ("unavailable", "Faktory persistence is unavailable.", true)
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
        "Get one model and its exact desired-revision source.",
        true,
        model_id_schema(),
    )
}
fn model_inspect_definition() -> McpToolDefinition {
    definition(
        "model.inspect",
        "Return one bounded technical projection from the current successful render.",
        true,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "projection": {
                    "type": "string",
                    "enum": ["isometric", "front", "back", "left", "right", "top", "bottom"],
                    "description": "Canonical technical projection."
                }
            },
            "required": ["model_id", "projection"],
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
        "Create a model with a caller-supplied kebab-case ID.",
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
                "source": {
                    "type": "string",
                    "minLength": 1,
                    "description": "UTF-8 Python source, limited to 1048576 bytes."
                }
            },
            "required": ["model_id", "name", "source"],
            "additionalProperties": false
        }),
    )
}
fn model_edit_definition() -> McpToolDefinition {
    definition(
        "model.edit",
        "Conditionally edit model metadata and exact source matches on the desired revision.",
        false,
        json!({
            "type": "object",
            "properties": {
                "model_id": model_id_property(),
                "expected_revision": {
                    "type": "string",
                    "pattern": "^[0-9a-f]{64}$",
                    "description": "Exact desired source revision to edit."
                },
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Display name, limited to 200 UTF-8 bytes."
                },
                "patches": {
                    "type": "array",
                    "minItems": 1,
                    "description": "Sequential exact patches; old must be non-empty and match exactly once at each step.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old": {"type": "string", "minLength": 1},
                            "new": {"type": "string"}
                        },
                        "required": ["old", "new"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["model_id", "expected_revision"],
            "anyOf": [
                {"required": ["name"]},
                {"required": ["patches"]}
            ],
            "additionalProperties": false
        }),
    )
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
            GeometryFactsRecord, GeometrySizeRecord, RenderedOutput, StoredRenderState,
            TechnicalProjectionImages, source_key,
        },
        render::RenderConfig,
        storage::{InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject},
    };

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
    struct BlockingFirstSourceGet {
        inner: InMemoryObjectStore,
        block_source_get: AtomicBool,
        source_get_started: Semaphore,
        release_source_get: Semaphore,
    }

    impl BlockingFirstSourceGet {
        fn new() -> Self {
            Self {
                inner: InMemoryObjectStore::default(),
                block_source_get: AtomicBool::new(false),
                source_get_started: Semaphore::new(0),
                release_source_get: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl ObjectStore for BlockingFirstSourceGet {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            if key.ends_with("/source.py") && self.block_source_get.swap(false, Ordering::SeqCst) {
                self.source_get_started.add_permits(1);
                self.release_source_get
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
        RenderedOutput {
            glb: Bytes::from_static(b"glb"),
            preview: Bytes::from_static(b"<svg></svg>"),
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            },
            projections: TechnicalProjectionImages::all(image),
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
    async fn model_get_returns_exact_source_with_matching_desired_revision() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let exact_source = "# caf\u{e9}\nresult = box";
        let created = repository
            .create_model("part", "Part", exact_source.as_bytes())
            .await
            .expect("create model");

        let (model, source) = get_model_with_source(&repository, "part")
            .await
            .expect("get model source");
        let tool_result = model_get_result(model, source);
        let output = tool_result.raw["structuredContent"].clone();
        let text = tool_result.raw["content"][0]["text"]
            .as_str()
            .expect("text content");

        assert_eq!(output["source"], exact_source);
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
    async fn model_get_returns_desired_source_instead_of_current_successful_source() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let created = repository
            .create_model("part", "Part", b"old source")
            .await
            .expect("create model");
        repository
            .complete_render(
                &created.id,
                &created.desired_source_revision,
                RenderedOutput {
                    glb: Bytes::from_static(b"glb"),
                    preview: Bytes::from_static(b"<svg></svg>"),
                    facts: GeometryFactsRecord {
                        volume_cubic_millimeters: 1.0,
                        size_millimeters: GeometrySizeRecord {
                            x: 1.0,
                            y: 1.0,
                            z: 1.0,
                        },
                    },
                    projections: TechnicalProjectionImages::all(Bytes::from_static(b"png")),
                },
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

        let (model, source) = get_model_with_source(&repository, "part")
            .await
            .expect("get desired source");

        assert_eq!(source, "desired source");
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

        let output = inspect_model(&repository, "part", TechnicalProjection::Front)
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

        let response = router.oneshot(request).await.expect("MCP response");
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

        let output = inspect_model(&repository, "part", TechnicalProjection::Top)
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

        let error = inspect_model(&repository, "part", TechnicalProjection::Bottom)
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
        let key = crate::model::projection_key(
            &rendered.id,
            &rendered.desired_source_revision,
            TechnicalProjection::Bottom,
        );
        let image = store.get(&key).await.expect("stored image");
        store.delete(&key, &image.etag).await.expect("remove image");
        let missing = inspect_model(&legacy_repository, "legacy", TechnicalProjection::Bottom)
            .await
            .expect_err("legacy image absent");
        assert_eq!(
            tool_error(missing).raw["structuredContent"]["error"]["code"],
            "not_found"
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

        let error = inspect_model(&repository, "corrupt", TechnicalProjection::Isometric)
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
    async fn model_get_pairs_loaded_metadata_with_its_immutable_source_during_edit() {
        let store = Arc::new(BlockingFirstSourceGet::new());
        let repository = Repository::new(store.clone(), 1);
        let created = repository
            .create_model("part", "Part", b"old source")
            .await
            .expect("create model");
        store.block_source_get.store(true, Ordering::SeqCst);

        let get_repository = repository.clone();
        let get = tokio::spawn(async move { get_model_with_source(&get_repository, "part").await });
        store
            .source_get_started
            .acquire()
            .await
            .expect("source-start semaphore open")
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
        store.release_source_get.add_permits(1);

        let (model, source) = get
            .await
            .expect("model get task")
            .expect("get model source");
        assert_eq!(
            model.desired_source_revision,
            created.desired_source_revision
        );
        assert_eq!(source, "old source");
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
    async fn model_get_maps_invalid_utf8_source_to_safe_invalid_state() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let created = repository
            .create_model("part", "Part", b"valid source")
            .await
            .expect("create model");
        store
            .put(
                &source_key("part", &created.desired_source_revision),
                Bytes::from_static(b"\xff\xfe"),
                PutCondition::Any,
            )
            .await
            .expect("corrupt source");

        let error = get_model_with_source(&repository, "part")
            .await
            .expect_err("invalid UTF-8 must fail");
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
    async fn model_get_distinguishes_missing_model_from_missing_desired_source() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);

        let absent = get_model_with_source(&repository, "absent")
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
        let key = source_key("part", &created.desired_source_revision);
        let source = store.get(&key).await.expect("stored source");
        store
            .delete(&key, &source.etag)
            .await
            .expect("delete desired source");

        let missing_source = get_model_with_source(&repository, "part")
            .await
            .expect_err("missing desired source must fail");
        assert_eq!(missing_source, RepositoryError::Corrupt);
        let output = tool_error(missing_source);
        assert_eq!(
            output.raw["structuredContent"]["error"]["code"],
            "invalid_state"
        );
        assert_eq!(output.raw["content"][0]["text"], "Stored state is invalid.");
    }

    #[test]
    fn create_and_edit_schemas_describe_slug_and_exact_patch_contracts() {
        let create = model_create_definition();
        assert_eq!(create.name, "model.create");
        assert_eq!(
            create.input_schema["properties"]["model_id"]["pattern"],
            "^[a-z0-9]+(-[a-z0-9]+)*$"
        );
        assert_eq!(
            create.input_schema["required"],
            json!(["model_id", "name", "source"])
        );

        let edit = model_edit_definition();
        assert_eq!(edit.name, "model.edit");
        assert_eq!(edit.input_schema["properties"]["patches"]["minItems"], 1);
        assert_eq!(
            edit.input_schema["properties"]["patches"]["items"]["properties"]["old"]["minLength"],
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
        assert_eq!(inspect.input_schema["additionalProperties"], false);
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
                    patches: None,
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
                source: "source".to_owned(),
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
