//! Fail-closed authentication configuration and secret handling.

use std::{fmt, sync::Arc};

use crate::production::ProductionAuthConfig;

#[derive(Clone)]
pub struct Secret(Arc<str>);

impl Secret {
    pub fn new(value: String) -> Result<Self, AuthError> {
        if value.len() < 24 || value.len() > 4096 {
            return Err(AuthError::InvalidConfiguration);
        }
        Ok(Self(Arc::from(value)))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

#[derive(Clone)]
pub struct S3AccessKeyId(Arc<str>);

impl S3AccessKeyId {
    pub fn new(value: String) -> Result<Self, AuthError> {
        if value.is_empty() || value.len() > 4096 {
            return Err(AuthError::InvalidConfiguration);
        }
        Ok(Self(Arc::from(value)))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for S3AccessKeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("S3AccessKeyId([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub enum AuthConfig {
    Disabled,
    Production(Box<ProductionAuthConfig>),
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum AuthError {
    #[error("authentication configuration is invalid")]
    InvalidConfiguration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted() {
        let secret =
            Secret::new("a-very-long-development-secret".to_owned()).expect("valid secret");
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
    }

    #[test]
    fn s3_access_key_ids_accept_obc_lengths_and_are_redacted() {
        for value in ["0123456789abcdef", "0123456789abcdefghij"] {
            let access_key = S3AccessKeyId::new(value.to_owned()).expect("valid access key ID");
            assert_eq!(access_key.expose(), value);
            assert_eq!(format!("{access_key:?}"), "S3AccessKeyId([REDACTED])");
        }
    }

    #[test]
    fn s3_access_key_ids_reject_empty_and_oversized_values() {
        for value in [String::new(), "x".repeat(4097)] {
            assert_eq!(
                S3AccessKeyId::new(value).expect_err("invalid access key ID"),
                AuthError::InvalidConfiguration
            );
        }
    }
}
