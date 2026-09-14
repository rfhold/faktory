//! PostgreSQL-backed browser OIDC and hosted MCP OAuth runtime.

use std::{collections::HashSet, fmt, fs, sync::Arc, time::Duration};

use axum::response::IntoResponse as _;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kuri_server::{
    access_auth::{AccessAuthConfig, AccessAuthHttpClient, AccessAuthenticator, OidcConfig},
    persistence::{DatabaseBackend, DatabaseConfig, Persistence},
};
use mcp::{
    HardenedOAuthClientMetadataFetcher, McpOAuthEntropy, McpOAuthSecret, McpPrincipalId,
    McpSystemOAuthClock, OAuthAuthorizationServer, OAuthAuthorizationServerConfig,
    OAuthAuthorizationStore as _, OAuthClientRegistrationOptions, OAuthConsentDecisionEvidence,
    OAuthConsentHandler, OAuthConsentModel, OAuthConsentPresentation, OAuthResource,
    OAuthSigningKeyState, OidcEndpointPolicy, OidcPrincipalMapper, OidcPrincipalMapping,
    OidcResourceOwnerAuthenticator, OidcResourceOwnerConfig, OidcVerifiedIdentity,
    PostgresOAuthAuthorizationStore, PostgresOidcResourceOwnerStore,
    TrustedPrivateOAuthCimdDestinationPolicy, VersionedOAuthWrappingKeyring, server::BoxFuture,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use url::Url;

use crate::Secret;

const MCP_SCOPE: &str = "faktory:use";
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CIMD_TRUSTED_ORIGINS: usize = 16;
const MAX_CIMD_TRUSTED_ORIGIN_BYTES: usize = 512;
const MAX_CIMD_TRUSTED_ORIGINS_BYTES: usize = 4_096;

#[derive(Clone)]
pub struct ProductionAuthConfig {
    pub database_url: Secret,
    pub public_base_url: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: Secret,
    pub session_ttl: Duration,
    pub oauth_access_token_ttl: Duration,
    pub oauth_refresh_token_ttl: Duration,
    pub oauth_refresh_family_ttl: Duration,
    pub oauth_code_ttl: Duration,
    pub oauth_wrapping_keys_file: String,
    pub allow_dynamic_registration: bool,
    pub allow_cimd: bool,
    pub cimd_trusted_private_origins: Vec<Url>,
    pub allow_loopback_redirects: bool,
}

impl fmt::Debug for ProductionAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionAuthConfig")
            .field("public_base_url", &self.public_base_url)
            .field("oidc_issuer", &self.oidc_issuer)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("session_ttl", &self.session_ttl)
            .field("oauth_access_token_ttl", &self.oauth_access_token_ttl)
            .field("oauth_refresh_token_ttl", &self.oauth_refresh_token_ttl)
            .field("oauth_refresh_family_ttl", &self.oauth_refresh_family_ttl)
            .field("oauth_code_ttl", &self.oauth_code_ttl)
            .field("oauth_wrapping_keys_file", &self.oauth_wrapping_keys_file)
            .field(
                "allow_dynamic_registration",
                &self.allow_dynamic_registration,
            )
            .field("allow_cimd", &self.allow_cimd)
            .field("allow_loopback_redirects", &self.allow_loopback_redirects)
            .finish_non_exhaustive()
    }
}

impl ProductionAuthConfig {
    pub fn validate(&self) -> Result<(), String> {
        let public = secure_origin("public base URL", &self.public_base_url)?;
        let issuer = secure_origin("OIDC issuer", &self.oidc_issuer)?;
        if issuer.query().is_some() || issuer.fragment().is_some() {
            return Err("OIDC issuer must not contain a query or fragment".to_owned());
        }
        if self.oidc_client_id.trim() != self.oidc_client_id
            || self.oidc_client_id.is_empty()
            || self.session_ttl.is_zero()
            || self.session_ttl > Duration::from_hours(720)
            || self.oauth_access_token_ttl.is_zero()
            || self.oauth_refresh_token_ttl.is_zero()
            || self.oauth_refresh_family_ttl < self.oauth_refresh_token_ttl
            || self.oauth_code_ttl.is_zero()
            || validate_cimd_trusted_private_origins(&self.cimd_trusted_private_origins).is_err()
            || public.path() != "/"
            || public.query().is_some()
            || public.fragment().is_some()
        {
            return Err("production authentication configuration is invalid".to_owned());
        }
        Ok(())
    }

    #[must_use]
    pub fn oauth_issuer(&self) -> String {
        format!("{}oauth", self.public_base_url)
    }

    #[must_use]
    pub fn mcp_resource(&self) -> String {
        format!("{}mcp", self.public_base_url)
    }

    #[must_use]
    pub fn browser_oidc_callback(&self) -> String {
        format!("{}oidc/callback", self.public_base_url)
    }

    #[must_use]
    pub fn mcp_oidc_callback(&self) -> String {
        format!("{}oauth/oidc/callback", self.public_base_url)
    }

    #[must_use]
    pub fn mcp_authorization_endpoint(&self) -> String {
        format!("{}oauth/authorize", self.public_base_url)
    }
}

pub struct ProductionAuthRuntime {
    pub browser: AccessAuthenticator,
    pub oauth: OAuthAuthorizationServer,
    pub oidc_owner: OidcResourceOwnerAuthenticator,
    persistence: Persistence,
    pool: PgPool,
}

impl fmt::Debug for ProductionAuthRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionAuthRuntime")
            .finish_non_exhaustive()
    }
}

impl ProductionAuthRuntime {
    #[allow(clippy::too_many_lines)]
    pub async fn initialize(config: &ProductionAuthConfig) -> Result<Self, String> {
        config.validate()?;
        let database = DatabaseConfig::from_url(config.database_url.expose())
            .map_err(|_| "invalid PostgreSQL configuration".to_owned())?;
        if database.backend() != DatabaseBackend::PostgreSql {
            return Err("production authentication requires PostgreSQL".to_owned());
        }
        let persistence = Persistence::initialize(database).await.map_err(|_| {
            tracing::error!("failed to initialize browser-session persistence");
            "failed to initialize browser-session persistence".to_owned()
        })?;
        let pool = persistence
            .postgres_pool()
            .ok_or_else(|| "production authentication requires PostgreSQL".to_owned())?
            .clone();
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|_| "failed to apply Faktory authentication migrations".to_owned())?;
        let browser_oidc = OidcConfig::new(
            config.oidc_issuer.clone(),
            config.oidc_client_id.clone(),
            Some(config.oidc_client_secret.expose().to_owned()),
            config.browser_oidc_callback(),
            config.oidc_client_id.clone(),
            ["openid", "profile", "email"].map(str::to_owned).to_vec(),
            config.session_ttl,
            false,
            false,
        )
        .map_err(|_| "invalid browser OIDC configuration".to_owned())?;
        let client = AccessAuthHttpClient::build(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(10)),
        )
        .map_err(|_| "failed to build OIDC client".to_owned())?;
        let browser = AccessAuthenticator::initialize(
            AccessAuthConfig::oidc(browser_oidc),
            &persistence,
            client,
        )
        .await
        .map_err(|_| {
            tracing::error!("failed to initialize browser OIDC");
            "failed to initialize browser OIDC".to_owned()
        })?;

        let store = Arc::new(
            PostgresOAuthAuthorizationStore::from_pool(pool.clone())
                .await
                .map_err(|_| "failed to initialize OAuth persistence".to_owned())?,
        );
        let oidc_store = Arc::new(
            PostgresOidcResourceOwnerStore::from_pool(pool.clone())
                .await
                .map_err(|_| "failed to initialize OAuth OIDC persistence".to_owned())?,
        );
        let entropy: Arc<dyn McpOAuthEntropy> = Arc::new(SystemEntropy);
        let owner_config = OidcResourceOwnerConfig::new(
            config.oidc_issuer.clone(),
            config.oidc_client_id.clone(),
            Some(McpOAuthSecret::new(
                config.oidc_client_secret.expose().to_owned(),
            )),
            config.mcp_oidc_callback(),
            config.mcp_authorization_endpoint(),
            ["openid", "profile", "email"].map(str::to_owned).to_vec(),
            config.oauth_code_ttl.min(Duration::from_mins(10)),
            OidcEndpointPolicy::HttpsOnly,
        )
        .map_err(|_| "invalid OAuth OIDC configuration".to_owned())?;
        let oidc_owner = OidcResourceOwnerAuthenticator::discover(
            owner_config,
            oidc_store,
            Arc::new(StablePrincipalMapper),
            Arc::new(McpSystemOAuthClock),
            entropy.clone(),
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(10)),
        )
        .await
        .map_err(|_| "failed to initialize OAuth OIDC".to_owned())?;
        let mut policy = OAuthAuthorizationServerConfig::new(
            config.oauth_issuer(),
            vec![OAuthResource {
                resource: config.mcp_resource(),
                scopes: vec![MCP_SCOPE.to_owned()],
            }],
        );
        policy.authorization_code_lifetime = config.oauth_code_ttl;
        policy.access_token_lifetime = config.oauth_access_token_ttl;
        policy.refresh_token_lifetime = config.oauth_refresh_token_ttl;
        policy.refresh_family_lifetime = config.oauth_refresh_family_ttl;
        policy.signing_verification_overlap = policy.signing_verification_overlap.max(
            policy
                .access_token_lifetime
                .checked_add(policy.clock_skew)
                .ok_or_else(|| "invalid OAuth lifetime".to_owned())?,
        );
        if config.allow_dynamic_registration {
            policy.registration_endpoint =
                Some(format!("{}oauth/register", config.public_base_url));
        }
        let keyring = load_keyring(&config.oauth_wrapping_keys_file)?;
        let consent = Arc::new(ExactConsent {
            resource: config.mcp_resource(),
        });
        let mut oauth = OAuthAuthorizationServer::new(
            policy,
            store.clone(),
            Arc::new(oidc_owner.clone()),
            consent,
            Arc::new(McpSystemOAuthClock),
            entropy,
            keyring,
        )
        .map_err(|_| "invalid hosted OAuth configuration".to_owned())?;
        if let Some(options) = client_registration_options(config)? {
            oauth = oauth
                .with_client_registration(options)
                .map_err(|_| "invalid OAuth registration configuration".to_owned())?;
        }
        initialize_signing_key(&pool, store.as_ref(), &oauth, &config.oauth_issuer()).await?;
        Ok(Self {
            browser,
            oauth,
            oidc_owner,
            persistence,
            pool,
        })
    }

    pub async fn ready(&self) -> bool {
        tokio::time::timeout(READINESS_TIMEOUT, async {
            self.persistence.ready().await.is_ok()
                && sqlx::query_scalar::<_, i32>("SELECT 1")
                    .fetch_one(&self.pool)
                    .await
                    .is_ok()
                && self.oauth.validate_signing_key_readiness().await.is_ok()
        })
        .await
        .unwrap_or(false)
    }
}

fn client_registration_options(
    config: &ProductionAuthConfig,
) -> Result<Option<OAuthClientRegistrationOptions>, String> {
    if !(config.allow_dynamic_registration || config.allow_cimd || config.allow_loopback_redirects)
    {
        return Ok(None);
    }
    let metadata_fetcher = if config.allow_cimd {
        let mut fetcher = HardenedOAuthClientMetadataFetcher::production()
            .with_loopback_redirects(config.allow_loopback_redirects);
        if !config.cimd_trusted_private_origins.is_empty() {
            let destination_policy = TrustedPrivateOAuthCimdDestinationPolicy::new(
                config.cimd_trusted_private_origins.clone(),
            )
            .map_err(|_| "invalid trusted CIMD destination policy".to_owned())?;
            fetcher = fetcher.with_destination_policy(Arc::new(destination_policy));
        }
        Some(Arc::new(fetcher) as Arc<_>)
    } else {
        None
    };
    Ok(Some(OAuthClientRegistrationOptions {
        metadata_fetcher,
        dynamic_registration: config.allow_dynamic_registration,
        allow_loopback_redirects: config.allow_loopback_redirects,
        source_resolver: None,
        ..OAuthClientRegistrationOptions::default()
    }))
}

/// Parse bounded, comma-separated trusted private CIMD origins.
///
/// # Errors
///
/// Returns an error unless every unique value is a credential-free HTTPS root origin.
pub fn parse_cimd_trusted_private_origins(value: &str) -> Result<Vec<Url>, String> {
    if value.is_empty() || value.len() > MAX_CIMD_TRUSTED_ORIGINS_BYTES {
        return Err("FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid".to_owned());
    }
    let mut origins = Vec::new();
    for (index, entry) in value.split(',').enumerate() {
        let entry = entry.trim();
        if entry.is_empty()
            || entry.len() > MAX_CIMD_TRUSTED_ORIGIN_BYTES
            || index >= MAX_CIMD_TRUSTED_ORIGINS
        {
            return Err("FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid".to_owned());
        }
        let origin = Url::parse(entry)
            .map_err(|_| "FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid".to_owned())?;
        validate_cimd_trusted_private_origins(std::slice::from_ref(&origin))?;
        if !origins
            .iter()
            .any(|existing: &Url| existing.origin() == origin.origin())
        {
            origins.push(origin);
        }
    }
    Ok(origins)
}

fn validate_cimd_trusted_private_origins(origins: &[Url]) -> Result<(), String> {
    if origins.len() > MAX_CIMD_TRUSTED_ORIGINS
        || origins
            .iter()
            .map(|origin| origin.as_str().len())
            .sum::<usize>()
            > MAX_CIMD_TRUSTED_ORIGINS_BYTES
        || origins
            .iter()
            .any(|origin| origin.as_str().len() > MAX_CIMD_TRUSTED_ORIGIN_BYTES)
    {
        return Err("FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid".to_owned());
    }
    TrustedPrivateOAuthCimdDestinationPolicy::new(origins.iter().cloned())
        .map(|_| ())
        .map_err(|_| "FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid".to_owned())
}

async fn initialize_signing_key(
    pool: &PgPool,
    store: &PostgresOAuthAuthorizationStore,
    server: &OAuthAuthorizationServer,
    issuer: &str,
) -> Result<(), String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| "failed to lock OAuth signing key".to_owned())?;
    sqlx::query("SELECT pg_advisory_xact_lock(1213155660, 1331053396)")
        .execute(&mut *transaction)
        .await
        .map_err(|_| "failed to lock OAuth signing key".to_owned())?;
    if store
        .active_signing_key(issuer.to_owned())
        .await
        .map_err(|_| "failed to inspect OAuth signing key".to_owned())?
        .is_none()
    {
        let generated = server
            .generate_signing_key_candidate(true)
            .await
            .map_err(|_| "failed to generate OAuth signing key".to_owned())?;
        if generated.state != OAuthSigningKeyState::Active
            && !server
                .activate_signing_key(generated.key_id)
                .await
                .map_err(|_| "failed to activate OAuth signing key".to_owned())?
        {
            return Err("failed to activate OAuth signing key".to_owned());
        }
    }
    server
        .validate_signing_key_readiness()
        .await
        .map_err(|_| "OAuth signing key is not ready".to_owned())?;
    transaction
        .commit()
        .await
        .map_err(|_| "failed to commit OAuth signing key".to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyringFile {
    schema_version: u8,
    active: String,
    keys: Vec<KeyringKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyringKey {
    id: String,
    key: String,
}

fn load_keyring(path: &str) -> Result<Arc<VersionedOAuthWrappingKeyring>, String> {
    let bytes = fs::read(path).map_err(|_| "failed to read OAuth wrapping keyring".to_owned())?;
    let file: KeyringFile =
        serde_json::from_slice(&bytes).map_err(|_| "invalid OAuth wrapping keyring".to_owned())?;
    if file.schema_version != 1 || file.keys.is_empty() {
        return Err("unsupported OAuth wrapping keyring".to_owned());
    }
    let keys = file
        .keys
        .into_iter()
        .map(|entry| {
            URL_SAFE_NO_PAD
                .decode(entry.key)
                .map(|key| (entry.id, key))
                .map_err(|_| "invalid OAuth wrapping keyring".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    VersionedOAuthWrappingKeyring::new(file.active, keys)
        .map(Arc::new)
        .map_err(|_| "invalid OAuth wrapping keyring".to_owned())
}

fn secure_origin(name: &str, value: &str) -> Result<Url, String> {
    let parsed = Url::parse(value).map_err(|_| format!("invalid {name}"))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{name} must be an HTTPS URL without userinfo"));
    }
    Ok(parsed)
}

struct SystemEntropy;

impl McpOAuthEntropy for SystemEntropy {
    fn fill_bytes(&self, output: &mut [u8]) -> mcp::Result<()> {
        getrandom::fill(output).map_err(|_| mcp::Error::protocol("system entropy unavailable"))
    }
}

struct StablePrincipalMapper;

impl OidcPrincipalMapper for StablePrincipalMapper {
    fn map(&self, identity: OidcVerifiedIdentity) -> BoxFuture<OidcPrincipalMapping> {
        Box::pin(async move {
            let mut digest = Sha256::new();
            digest.update(identity.issuer.as_bytes());
            digest.update([0]);
            digest.update(identity.subject.as_bytes());
            McpPrincipalId::new(format!(
                "authentik:{}",
                URL_SAFE_NO_PAD.encode(digest.finalize())
            ))
            .map_or(
                OidcPrincipalMapping::Denied,
                OidcPrincipalMapping::Principal,
            )
        })
    }
}

struct ExactConsent {
    resource: String,
}

impl OAuthConsentHandler for ExactConsent {
    fn present(&self, model: OAuthConsentModel) -> BoxFuture<OAuthConsentPresentation> {
        let approved = model.resource == self.resource
            && model.requested_scopes.iter().collect::<HashSet<_>>()
                == HashSet::from([&MCP_SCOPE.to_owned()]);
        Box::pin(async move {
            if approved {
                OAuthConsentPresentation::Approved
            } else {
                OAuthConsentPresentation::Response(
                    axum::http::StatusCode::FORBIDDEN.into_response(),
                )
            }
        })
    }

    fn validate_decision(&self, _: OAuthConsentDecisionEvidence) -> BoxFuture<bool> {
        Box::pin(async { false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_urls_and_lifetimes_are_strict() {
        let config = ProductionAuthConfig {
            database_url: Secret::new("postgres://user:long-password@db/faktory".to_owned())
                .expect("secret"),
            public_base_url: "https://faktory.example/".to_owned(),
            oidc_issuer: "https://auth.example/application/o/faktory/".to_owned(),
            oidc_client_id: "faktory".to_owned(),
            oidc_client_secret: Secret::new("long-production-client-secret".to_owned())
                .expect("secret"),
            session_ttl: Duration::from_hours(8),
            oauth_access_token_ttl: Duration::from_mins(15),
            oauth_refresh_token_ttl: Duration::from_hours(24),
            oauth_refresh_family_ttl: Duration::from_hours(720),
            oauth_code_ttl: Duration::from_mins(5),
            oauth_wrapping_keys_file: "/run/secrets/faktory-oauth-keys".to_owned(),
            allow_dynamic_registration: true,
            allow_cimd: false,
            cimd_trusted_private_origins: Vec::new(),
            allow_loopback_redirects: false,
        };
        assert!(config.validate().is_ok());
        assert_eq!(config.oauth_issuer(), "https://faktory.example/oauth");
        assert_eq!(config.mcp_resource(), "https://faktory.example/mcp");
        assert_eq!(
            config.browser_oidc_callback(),
            "https://faktory.example/oidc/callback"
        );
        assert_eq!(
            config.mcp_oidc_callback(),
            "https://faktory.example/oauth/oidc/callback"
        );
        assert_eq!(
            config.mcp_authorization_endpoint(),
            "https://faktory.example/oauth/authorize"
        );
        let mut invalid = config;
        for url in ["http://faktory.example/", "http://localhost:8080/"] {
            invalid.public_base_url = url.to_owned();
            assert!(invalid.validate().is_err(), "accepted {url}");
        }
    }

    #[test]
    fn production_rejects_insecure_urls() {
        let mut config = ProductionAuthConfig {
            database_url: Secret::new("postgres://user:long-password@db/faktory".to_owned())
                .expect("secret"),
            public_base_url: "http://localhost:8080/".to_owned(),
            oidc_issuer: "http://127.0.0.1:9000/application/o/faktory/".to_owned(),
            oidc_client_id: "faktory".to_owned(),
            oidc_client_secret: Secret::new("long-production-client-secret".to_owned())
                .expect("secret"),
            session_ttl: Duration::from_hours(8),
            oauth_access_token_ttl: Duration::from_mins(15),
            oauth_refresh_token_ttl: Duration::from_hours(24),
            oauth_refresh_family_ttl: Duration::from_hours(720),
            oauth_code_ttl: Duration::from_mins(5),
            oauth_wrapping_keys_file: "/run/secrets/faktory-oauth-keys".to_owned(),
            allow_dynamic_registration: true,
            allow_cimd: false,
            cimd_trusted_private_origins: Vec::new(),
            allow_loopback_redirects: true,
        };
        assert!(config.validate().is_err());

        config.public_base_url = "https://faktory.example/".to_owned();
        assert!(config.validate().is_err());
    }

    #[test]
    fn cimd_is_independent_from_dcr_and_loopback_support() {
        let mut config = valid_config();
        config.allow_dynamic_registration = true;
        config.allow_loopback_redirects = true;
        config.validate().expect("DCR and loopback configuration");
        let options = client_registration_options(&config)
            .expect("registration options")
            .expect("enabled registration");
        assert!(options.dynamic_registration);
        assert!(options.allow_loopback_redirects);
        assert!(options.metadata_fetcher.is_none());

        config.allow_dynamic_registration = false;
        config.allow_loopback_redirects = false;
        config.allow_cimd = true;
        config.validate().expect("CIMD-only configuration");
        let options = client_registration_options(&config)
            .expect("registration options")
            .expect("enabled CIMD");
        assert!(!options.dynamic_registration);
        assert!(options.metadata_fetcher.is_some());

        config.allow_cimd = false;
        config.allow_loopback_redirects = true;
        config.validate().expect("loopback-only configuration");
        let options = client_registration_options(&config)
            .expect("registration options")
            .expect("enabled loopback support");
        assert!(!options.dynamic_registration);
        assert!(options.allow_loopback_redirects);
        assert!(options.metadata_fetcher.is_none());

        config.allow_loopback_redirects = false;
        config
            .validate()
            .expect("disabled registration configuration");
        assert!(
            client_registration_options(&config)
                .expect("registration options")
                .is_none()
        );
    }

    #[test]
    fn trusted_cimd_origins_are_bounded_validated_and_deduplicated() {
        let origins = parse_cimd_trusted_private_origins(
            "https://PRIVATE.example:443, https://private.example/,https://other.example:8443",
        )
        .expect("trusted origins");
        assert_eq!(origins.len(), 2);

        for invalid in [
            "http://private.example",
            "https://user@private.example",
            "https://private.example/path",
            "https://private.example?token=secret-value",
            "https://localhost",
            "https://127.0.0.1",
            "https://169.254.169.254",
            "https://[::1]",
            "https://private.example,",
        ] {
            let error = parse_cimd_trusted_private_origins(invalid).expect_err("invalid origin");
            assert_eq!(
                error,
                "FAKTORY_OAUTH_CIMD_TRUSTED_PRIVATE_ORIGINS is invalid"
            );
            assert!(!error.contains(invalid));
            assert!(!error.contains("secret-value"));
        }
        let too_many = std::iter::repeat_n("https://private.example", 17)
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_cimd_trusted_private_origins(&too_many).is_err());
        assert!(parse_cimd_trusted_private_origins(&"x".repeat(4_097)).is_err());
    }

    #[test]
    fn cimd_diagnostics_do_not_expose_trusted_origins() {
        let mut config = valid_config();
        config.allow_cimd = true;
        config.cimd_trusted_private_origins =
            vec![Url::parse("https://private.internal.example/").expect("URL")];
        let diagnostic = format!("{config:?}");
        assert!(diagnostic.contains("allow_cimd: true"));
        assert!(!diagnostic.contains("private.internal.example"));
    }

    fn valid_config() -> ProductionAuthConfig {
        ProductionAuthConfig {
            database_url: Secret::new("postgres://user:long-password@db/faktory".to_owned())
                .expect("secret"),
            public_base_url: "https://faktory.example/".to_owned(),
            oidc_issuer: "https://auth.example/application/o/faktory/".to_owned(),
            oidc_client_id: "faktory".to_owned(),
            oidc_client_secret: Secret::new("long-production-client-secret".to_owned())
                .expect("secret"),
            session_ttl: Duration::from_hours(8),
            oauth_access_token_ttl: Duration::from_mins(15),
            oauth_refresh_token_ttl: Duration::from_hours(24),
            oauth_refresh_family_ttl: Duration::from_hours(720),
            oauth_code_ttl: Duration::from_mins(5),
            oauth_wrapping_keys_file: "/run/secrets/faktory-oauth-keys".to_owned(),
            allow_dynamic_registration: true,
            allow_cimd: false,
            cimd_trusted_private_origins: Vec::new(),
            allow_loopback_redirects: false,
        }
    }
}
