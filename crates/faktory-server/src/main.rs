//! Faktory server executable.

use std::{env, error::Error, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use faktory_server::{
    AppConfig, AuthConfig, AwsObjectStore, ProductionAuthConfig, RenderConfig, S3AccessKeyId,
    Secret, build_runtime, observability, profiling,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let telemetry_config = observability::TelemetryConfig::from_env()?;
    let observability = observability::init(&telemetry_config)?;
    let profiling = profiling::init(&telemetry_config)?;
    let auth = auth_from_env()?;
    let store = AwsObjectStore::garage(
        required("FAKTORY_S3_ENDPOINT")?,
        env::var("FAKTORY_S3_REGION").unwrap_or_else(|_| "garage".to_owned()),
        required("FAKTORY_S3_BUCKET")?,
        S3AccessKeyId::new(required("FAKTORY_S3_ACCESS_KEY")?)?,
        Secret::new(required("FAKTORY_S3_SECRET_KEY")?)?,
    )
    .await?;
    let command: Vec<String> = serde_json::from_str(&required("FAKTORY_RENDER_COMMAND_JSON")?)
        .map_err(|_| "FAKTORY_RENDER_COMMAND_JSON must be a JSON string array")?;
    let runtime = build_runtime(
        Arc::new(store),
        AppConfig {
            auth,
            public_base_url: required("FAKTORY_PUBLIC_BASE_URL")?,
            static_directory: env::var_os("FAKTORY_STATIC_DIR").map(PathBuf::from),
            render: RenderConfig {
                command,
                queue_capacity: parse("FAKTORY_RENDER_QUEUE_CAPACITY", 16)?,
                concurrency: parse("FAKTORY_RENDER_CONCURRENCY", 1)?,
                timeout: Duration::from_secs(parse("FAKTORY_RENDER_TIMEOUT_SECONDS", 120)?),
                max_output_bytes: parse("FAKTORY_RENDER_MAX_OUTPUT_BYTES", 64 * 1024 * 1024)?,
            },
            watch_capacity: parse("FAKTORY_WATCH_CAPACITY", 128)?,
        },
    )
    .await
    .map_err(std::io::Error::other)?;
    let address: SocketAddr = env::var("FAKTORY_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let server_result = axum::serve(listener, runtime.router())
        .with_graceful_shutdown(shutdown_signal())
        .await;
    tracing::info!("service shutdown started");
    profiling.shutdown();
    tracing::info!("service shutdown complete");
    observability.shutdown();
    server_result?;
    Ok(())
}

fn auth_from_env() -> Result<AuthConfig, Box<dyn Error + Send + Sync>> {
    match auth_mode(env::var("FAKTORY_AUTH_MODE"))? {
        AuthMode::Disabled => Ok(AuthConfig::Disabled),
        AuthMode::Production => production_auth_from_env(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthMode {
    Disabled,
    Production,
}

fn auth_mode(
    value: Result<String, env::VarError>,
) -> Result<AuthMode, Box<dyn Error + Send + Sync>> {
    match value {
        Ok(value) if value == "disabled" => Ok(AuthMode::Disabled),
        Ok(value) if value == "production" => Ok(AuthMode::Production),
        Ok(value) => Err(format!("unsupported FAKTORY_AUTH_MODE: {value}").into()),
        Err(env::VarError::NotPresent) => Ok(AuthMode::Production),
        Err(error) => Err(error.into()),
    }
}

fn production_auth_from_env() -> Result<AuthConfig, Box<dyn Error + Send + Sync>> {
    Ok(AuthConfig::Production(Box::new(ProductionAuthConfig {
        database_url: Secret::new(required("FAKTORY_DATABASE_URL")?)?,
        public_base_url: normalized_base_url(required("FAKTORY_PUBLIC_BASE_URL")?),
        oidc_issuer: required("FAKTORY_OIDC_ISSUER")?,
        oidc_client_id: required("FAKTORY_OIDC_CLIENT_ID")?,
        oidc_client_secret: Secret::new(required("FAKTORY_OIDC_CLIENT_SECRET")?)?,
        session_ttl: Duration::from_secs(parse("FAKTORY_SESSION_TTL_SECONDS", 28_800)?),
        oauth_access_token_ttl: Duration::from_secs(parse(
            "FAKTORY_OAUTH_ACCESS_TOKEN_TTL_SECONDS",
            900,
        )?),
        oauth_refresh_token_ttl: Duration::from_secs(parse(
            "FAKTORY_OAUTH_REFRESH_TOKEN_TTL_SECONDS",
            86_400,
        )?),
        oauth_refresh_family_ttl: Duration::from_secs(parse(
            "FAKTORY_OAUTH_REFRESH_FAMILY_TTL_SECONDS",
            2_592_000,
        )?),
        oauth_code_ttl: Duration::from_secs(parse("FAKTORY_OAUTH_CODE_TTL_SECONDS", 300)?),
        oauth_wrapping_keys_file: required("FAKTORY_OAUTH_WRAPPING_KEYS_FILE")?,
        allow_dynamic_registration: parse_bool("FAKTORY_OAUTH_ALLOW_DCR", true)?,
        allow_loopback_redirects: parse_bool("FAKTORY_OAUTH_ALLOW_LOOPBACK_REDIRECTS", false)?,
    })))
}

fn normalized_base_url(mut value: String) -> String {
    if !value.ends_with('/') {
        value.push('/');
    }
    value
}

fn parse_bool(name: &str, default: bool) -> Result<bool, Box<dyn Error + Send + Sync>> {
    match env::var(name) {
        Ok(value) if value == "true" => Ok(true),
        Ok(value) if value == "false" => Ok(false),
        Ok(_) => Err(format!("{name} must be true or false").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn required(name: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    env::var(name).map_err(|_| format!("{name} is required").into())
}

fn parse<T>(name: &str, default: T) -> Result<T, Box<dyn Error + Send + Sync>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    env::var(name).map_or(Ok(default), |value| value.parse().map_err(Into::into))
}

async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install terminate handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = interrupt => {}, () = terminate => {} }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mode_is_explicit_and_exact() {
        assert_eq!(
            auth_mode(Ok("disabled".to_owned())).expect("disabled mode"),
            AuthMode::Disabled
        );
        for value in ["Disabled", "disabled ", "local-development", ""] {
            assert!(
                auth_mode(Ok(value.to_owned())).is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn absent_auth_mode_defaults_to_production() {
        assert_eq!(
            auth_mode(Err(env::VarError::NotPresent)).expect("default mode"),
            AuthMode::Production
        );
    }
}
