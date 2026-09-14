//! Faktory server runtime.

#![allow(missing_docs)]
#![allow(clippy::missing_errors_doc, clippy::needless_pass_by_value)]

pub mod app;
pub mod auth;
pub mod mcp;
pub mod model;
pub mod observability;
pub mod production;
pub mod profiling;
pub mod render;
pub mod service;
pub mod storage;

pub use app::{AppConfig, Runtime, build_runtime};
pub use auth::{AuthConfig, S3AccessKeyId, Secret};
pub use model::{ModelRecord, Repository, ViewRecord};
pub use production::ProductionAuthConfig;
pub use render::{RenderConfig, RenderQueue};
pub use storage::{AwsObjectStore, InMemoryObjectStore, ObjectStore};
