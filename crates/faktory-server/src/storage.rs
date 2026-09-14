//! Small object-storage boundary and S3 implementation.

use std::{collections::BTreeMap, fmt, sync::Arc};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::{Client, primitives::ByteStream};
use bytes::Bytes;
use tokio::sync::RwLock;

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
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes));
        let request = match condition {
            PutCondition::Any => request,
            PutCondition::Absent => request.if_none_match("*"),
            PutCondition::Matches(etag) => request.if_match(etag),
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
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .if_match(expected_etag)
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
