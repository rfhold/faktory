//! MCP tools over the same repository and render queue.

use std::sync::Arc;

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
    model::{ModelRecord, Repository, RepositoryError, SourcePatch},
    render::RenderQueue,
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
        match self.repository.get_model(&input.model_id).await {
            Ok(model) => Ok(result(json!({ "model": model.record }))),
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
    McpToolResult::new(json!({
        "content": [{ "type": "text", "text": "Request completed." }],
        "structuredContent": value
    }))
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
    definition("model.get", "Get one model.", true, model_id_schema())
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
    use bytes::Bytes;
    use tokio::sync::Semaphore;

    use super::*;
    use crate::{
        model::StoredRenderState,
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
