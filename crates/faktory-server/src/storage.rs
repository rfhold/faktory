//! Small object-storage boundary and S3 implementation.

use std::{collections::BTreeMap, fmt, sync::Arc};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::{Client, primitives::ByteStream};
use bytes::Bytes;
use tokio::sync::{Mutex, RwLock};

use crate::auth::{S3AccessKeyId, Secret};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObject {
    pub bytes: Bytes,
    pub etag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PutCondition {
    Any,
    Absent,
    Matches(String),
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum StorageError {
    #[error("object was not found")]
    NotFound,
    #[error("object write conflicted")]
    Conflict,
    #[error("object storage is unavailable")]
    Unavailable,
    #[error("stored object is invalid")]
    Invalid,
}

#[async_trait]
pub trait ObjectStore: Send + Sync + fmt::Debug {
    async fn get(&self, key: &str) -> Result<StoredObject, StorageError>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError>;
    async fn put(
        &self,
        key: &str,
        bytes: Bytes,
        condition: PutCondition,
    ) -> Result<String, StorageError>;
    async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError>;
    async fn ready(&self) -> Result<(), StorageError>;
}

#[derive(Clone)]
pub struct AwsObjectStore {
    client: Client,
    bucket: String,
    mutations: Arc<Mutex<()>>,
}

impl fmt::Debug for AwsObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AwsObjectStore")
            .finish_non_exhaustive()
    }
}

impl AwsObjectStore {
    pub async fn garage(
        endpoint: String,
        region: String,
        bucket: String,
        access_key: S3AccessKeyId,
        secret_key: Secret,
    ) -> Result<Self, StorageError> {
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            return Err(StorageError::Invalid);
        }
        let credentials = Credentials::new(
            access_key.expose(),
            secret_key.expose(),
            None,
            None,
            "faktory",
        );
        let shared = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(credentials)
            .endpoint_url(endpoint)
            .load()
            .await;
        let config = aws_sdk_s3::config::Builder::from(&shared)
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(config),
            bucket,
            mutations: Arc::new(Mutex::new(())),
        })
    }
}

#[async_trait]
impl ObjectStore for AwsObjectStore {
    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        let response =
            self.client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|error| {
                    if error.as_service_error().is_some_and(
                        aws_sdk_s3::operation::get_object::GetObjectError::is_no_such_key,
                    ) {
                        StorageError::NotFound
                    } else {
                        StorageError::Unavailable
                    }
                })?;
        let etag = response.e_tag().ok_or(StorageError::Invalid)?.to_owned();
        let bytes = response
            .body
            .collect()
            .await
            .map_err(|_| StorageError::Unavailable)?
            .into_bytes();
        Ok(StoredObject { bytes, etag })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut continuation = None;
        loop {
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(continuation)
                .send()
                .await
                .map_err(|_| StorageError::Unavailable)?;
            keys.extend(
                response
                    .contents()
                    .iter()
                    .filter_map(|item| item.key().map(str::to_owned)),
            );
            if !response.is_truncated().unwrap_or(false) {
                break;
            }
            continuation = response.next_continuation_token().map(str::to_owned);
        }
        Ok(keys)
    }

    async fn put(
        &self,
        key: &str,
        bytes: Bytes,
        condition: PutCondition,
    ) -> Result<String, StorageError> {
        let _guard = self.mutations.lock().await;
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes));
        let request = match condition {
            PutCondition::Any => request,
            PutCondition::Absent => request.if_none_match("*"),
            PutCondition::Matches(expected) => {
                if self.get(key).await?.etag != expected {
                    return Err(StorageError::Conflict);
                }
                request
            }
        };
        let response = request.send().await.map_err(|error| {
            if error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 412)
            {
                StorageError::Conflict
            } else {
                StorageError::Unavailable
            }
        })?;
        response
            .e_tag()
            .map(str::to_owned)
            .ok_or(StorageError::Invalid)
    }

    async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
        let _guard = self.mutations.lock().await;
        if self.get(key).await?.etag != expected_etag {
            return Err(StorageError::Conflict);
        }
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 412)
                {
                    StorageError::Conflict
                } else {
                    StorageError::Unavailable
                }
            })?;
        Ok(())
    }

    async fn ready(&self) -> Result<(), StorageError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use axum::{Router, body::Body, extract::State, response::Response, routing::any};
    use http::{HeaderMap, Method, StatusCode, header};
    use tokio::task::JoinHandle;

    use super::*;

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        method: Method,
        headers: HeaderMap,
        body: Bytes,
    }

    #[derive(Clone, Copy)]
    struct ScriptedResponse {
        status: StatusCode,
        etag: Option<&'static str>,
        body: &'static str,
    }

    #[derive(Clone)]
    struct ScriptedState {
        responses: Arc<Mutex<VecDeque<ScriptedResponse>>>,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        delay: Duration,
    }

    struct ScriptedServer {
        endpoint: String,
        state: ScriptedState,
        task: JoinHandle<()>,
    }

    impl ScriptedServer {
        async fn start(responses: Vec<ScriptedResponse>, delay: Duration) -> Self {
            let state = ScriptedState {
                responses: Arc::new(Mutex::new(responses.into())),
                requests: Arc::new(Mutex::new(Vec::new())),
                active: Arc::new(AtomicUsize::new(0)),
                max_active: Arc::new(AtomicUsize::new(0)),
                delay,
            };
            let router = Router::new()
                .route("/{*key}", any(scripted_s3))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind scripted S3 server");
            let address = listener.local_addr().expect("scripted S3 address");
            let task = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("serve scripted S3 responses");
            });
            Self {
                endpoint: format!("http://{address}"),
                state,
                task,
            }
        }

        async fn requests(&self) -> Vec<CapturedRequest> {
            self.state.requests.lock().await.clone()
        }
    }

    impl Drop for ScriptedServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn scripted_s3(
        State(state): State<ScriptedState>,
        method: Method,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
        state.max_active.fetch_max(active, Ordering::SeqCst);
        state.requests.lock().await.push(CapturedRequest {
            method,
            headers,
            body,
        });
        tokio::time::sleep(state.delay).await;
        let response = state
            .responses
            .lock()
            .await
            .pop_front()
            .expect("scripted response");
        state.active.fetch_sub(1, Ordering::SeqCst);

        let mut builder = Response::builder().status(response.status);
        if let Some(etag) = response.etag {
            builder = builder.header(header::ETAG, etag);
        }
        builder
            .body(Body::from(response.body))
            .expect("scripted response body")
    }

    const fn response(
        status: StatusCode,
        etag: Option<&'static str>,
        body: &'static str,
    ) -> ScriptedResponse {
        ScriptedResponse { status, etag, body }
    }

    async fn aws_store(server: &ScriptedServer) -> AwsObjectStore {
        AwsObjectStore::garage(
            server.endpoint.clone(),
            "us-east-1".to_owned(),
            "bucket".to_owned(),
            S3AccessKeyId::new("test-access-key".to_owned()).expect("test access key"),
            Secret::new("test-secret-at-least-24-bytes".to_owned()).expect("test secret"),
        )
        .await
        .expect("AWS object store")
    }

    #[tokio::test]
    async fn aws_matched_put_gets_etag_then_sends_unconditional_put() {
        let server = ScriptedServer::start(
            vec![
                response(StatusCode::OK, Some("\"current\""), "old"),
                response(StatusCode::OK, Some("\"updated\""), ""),
            ],
            Duration::ZERO,
        )
        .await;
        let store = aws_store(&server).await;

        assert_eq!(
            store
                .put(
                    "mutable",
                    Bytes::from_static(b"new"),
                    PutCondition::Matches("\"current\"".to_owned()),
                )
                .await
                .expect("matched PUT"),
            "\"updated\""
        );
        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[1].method, Method::PUT);
        assert_eq!(requests[1].body, Bytes::from_static(b"new"));
        assert!(!requests[1].headers.contains_key(header::IF_MATCH));
        assert!(!requests[1].headers.contains_key(header::IF_NONE_MATCH));
    }

    #[tokio::test]
    async fn aws_matched_delete_gets_etag_then_sends_unconditional_delete() {
        let server = ScriptedServer::start(
            vec![
                response(StatusCode::OK, Some("\"current\""), "old"),
                response(StatusCode::NO_CONTENT, None, ""),
            ],
            Duration::ZERO,
        )
        .await;
        let store = aws_store(&server).await;

        store
            .delete("mutable", "\"current\"")
            .await
            .expect("matched DELETE");
        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[1].method, Method::DELETE);
        assert!(!requests[1].headers.contains_key(header::IF_MATCH));
    }

    #[tokio::test]
    async fn aws_matched_mutations_stop_on_stale_or_missing_objects() {
        let stale_server = ScriptedServer::start(
            vec![response(StatusCode::OK, Some("\"newer\""), "value")],
            Duration::ZERO,
        )
        .await;
        let stale_store = aws_store(&stale_server).await;
        assert_eq!(
            stale_store
                .put(
                    "mutable",
                    Bytes::from_static(b"stale"),
                    PutCondition::Matches("\"old\"".to_owned()),
                )
                .await,
            Err(StorageError::Conflict)
        );
        assert_eq!(stale_server.requests().await.len(), 1);

        let missing_server = ScriptedServer::start(
            vec![response(
                StatusCode::NOT_FOUND,
                None,
                "<Error><Code>NoSuchKey</Code></Error>",
            )],
            Duration::ZERO,
        )
        .await;
        let missing_store = aws_store(&missing_server).await;
        assert_eq!(
            missing_store.delete("missing", "\"old\"").await,
            Err(StorageError::NotFound)
        );
        assert_eq!(missing_server.requests().await.len(), 1);
    }

    #[tokio::test]
    async fn aws_absent_put_retains_atomic_header_and_maps_precondition_failure() {
        let server = ScriptedServer::start(
            vec![response(
                StatusCode::PRECONDITION_FAILED,
                None,
                "<Error><Code>PreconditionFailed</Code></Error>",
            )],
            Duration::ZERO,
        )
        .await;
        let store = aws_store(&server).await;

        assert_eq!(
            store
                .put(
                    "immutable",
                    Bytes::from_static(b"value"),
                    PutCondition::Absent,
                )
                .await,
            Err(StorageError::Conflict)
        );
        let requests = server.requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::PUT);
        assert_eq!(requests[0].headers.get(header::IF_NONE_MATCH).unwrap(), "*");
    }

    #[tokio::test]
    async fn aws_clones_serialize_concurrent_mutations() {
        let server = ScriptedServer::start(
            vec![
                response(StatusCode::OK, Some("\"one\""), ""),
                response(StatusCode::OK, Some("\"two\""), ""),
            ],
            Duration::from_millis(50),
        )
        .await;
        let first = aws_store(&server).await;
        let second = first.clone();

        let (first_result, second_result) = tokio::join!(
            first.put("one", Bytes::from_static(b"one"), PutCondition::Any),
            second.put("two", Bytes::from_static(b"two"), PutCondition::Any),
        );
        first_result.expect("first mutation");
        second_result.expect("second mutation");

        assert_eq!(server.requests().await.len(), 2);
        assert_eq!(server.state.max_active.load(Ordering::SeqCst), 1);
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryObjectStore {
    objects: Arc<RwLock<BTreeMap<String, StoredObject>>>,
}

#[async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        self.objects
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or(StorageError::NotFound)
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        Ok(self
            .objects
            .read()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    async fn put(
        &self,
        key: &str,
        bytes: Bytes,
        condition: PutCondition,
    ) -> Result<String, StorageError> {
        let mut objects = self.objects.write().await;
        match (&condition, objects.get(key)) {
            (PutCondition::Absent, Some(_)) => return Err(StorageError::Conflict),
            (PutCondition::Matches(expected), Some(found)) if expected != &found.etag => {
                return Err(StorageError::Conflict);
            }
            (PutCondition::Matches(_), None) => return Err(StorageError::NotFound),
            _ => {}
        }
        let etag = format!("\"{}\"", uuid::Uuid::new_v4());
        objects.insert(
            key.to_owned(),
            StoredObject {
                bytes,
                etag: etag.clone(),
            },
        );
        drop(objects);
        Ok(etag)
    }

    async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
        let mut objects = self.objects.write().await;
        let found = objects.get(key).ok_or(StorageError::NotFound)?;
        if found.etag != expected_etag {
            return Err(StorageError::Conflict);
        }
        objects.remove(key);
        drop(objects);
        Ok(())
    }

    async fn ready(&self) -> Result<(), StorageError> {
        Ok(())
    }
}
