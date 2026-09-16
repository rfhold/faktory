//! Destructive hard cutover to v2 model projects and model releases.

use crate::{
    model::RepositoryError,
    storage::{ObjectStore, StorageError},
};

pub const ID: &str = "0002-model-dependency-cutover";
pub const DESCRIPTION: &str = "delete pre-model-release product data";

pub(super) const PREFIXES: &[&str] = &[
    "models/",
    "libraries/",
    "system/library-rollouts/",
    "system/migrations/0001-model-project-bundles/items/",
    "system/migrations/0001-model-project-bundles/backup/",
];

pub async fn run(store: &dyn ObjectStore) -> Result<(), RepositoryError> {
    for prefix in PREFIXES {
        loop {
            let keys = store.list(prefix).await?;
            if keys.is_empty() {
                break;
            }
            for key in keys {
                match store.get(&key).await {
                    Ok(object) => match store.delete(&key, &object.etag).await {
                        Ok(()) | Err(StorageError::NotFound | StorageError::Conflict) => {}
                        Err(error) => return Err(error.into()),
                    },
                    Err(StorageError::NotFound) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    for prefix in PREFIXES {
        if !store.list(prefix).await?.is_empty() {
            return Err(RepositoryError::Conflict);
        }
    }
    Ok(())
}
