use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use anyhow::{Context, bail};
use base64::Engine as _;
use secrecy::SecretString;
use url::Url;

#[derive(Clone, Debug)]
pub struct Settings {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub database_identity_url: String,
    pub database_max_connections: u32,
    pub database_identity_max_connections: u32,
    pub database_analytics_max_connections: u32,
    pub database_analytics_statement_timeout: Duration,
    pub database_acquire_timeout: Duration,
    pub database_runtime_role: Option<String>,
    pub database_identity_role: Option<String>,
    pub redis_url: String,
    pub auth_jwt_issuer: String,
    pub auth_jwt_audience: String,
    pub auth_jwt_secret: SecretString,
    pub auth_jwt_lifetime_seconds: u32,
    pub mcp_allowed_hosts: Vec<String>,
    pub google_client_id: Option<String>,
    pub apple_client_id: Option<String>,
    /// Deprecated fallback; notification jobs should carry the Store URL.
    pub storefront_public_base_url: Url,
    pub resend_api_base_url: Url,
    pub stripe_api_base_url: Url,
    pub easypost_api_base_url: Url,
    pub analytics_meta_api_base_url: Url,
    pub provider_secret_key: SecretKey,
    pub media_storage: Option<MediaStorageSettings>,
    pub shopper_token_active_key_id: String,
    pub shopper_token_active_secret: String,
    pub shopper_token_previous_key: Option<(String, String)>,
    pub dependency_timeout: Duration,
    pub shutdown_drain_delay: Duration,
    pub shutdown_worker_timeout: Duration,
    pub log_filter: String,
    pub log_json: bool,
}

#[derive(Clone, Debug)]
pub struct MediaStorageSettings {
    pub endpoint_url: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub force_path_style: bool,
    pub public_base_url: Url,
}

/// 32 raw bytes (AES-256) used to encrypt/decrypt Provider Key secrets stored in PostgreSQL.
#[derive(Clone)]
pub struct SecretKey([u8; 32]);

impl SecretKey {
    pub fn from_base64(value: &str) -> anyhow::Result<Self> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .context("must be valid base64")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("must decode to exactly 32 bytes"))?;
        Ok(Self(bytes))
    }

    #[cfg(test)]
    pub fn from_raw(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(REDACTED)")
    }
}

impl Settings {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = required("DATABASE_URL")?;
        let database_identity_url =
            optional("DATABASE_IDENTITY_URL").unwrap_or_else(|| database_url.clone());
        let settings = Self {
            bind_addr: parse_or("APP_BIND_ADDR", "0.0.0.0:8080")?,
            database_url,
            database_identity_url,
            database_max_connections: parse_or("DATABASE_MAX_CONNECTIONS", "20")?,
            database_identity_max_connections: parse_or("DATABASE_IDENTITY_MAX_CONNECTIONS", "5")?,
            database_analytics_max_connections: parse_or(
                "DATABASE_ANALYTICS_MAX_CONNECTIONS",
                "3",
            )?,
            database_analytics_statement_timeout: Duration::from_millis(parse_or(
                "DATABASE_ANALYTICS_STATEMENT_TIMEOUT_MS",
                "2000",
            )?),
            database_acquire_timeout: Duration::from_millis(parse_or(
                "DATABASE_ACQUIRE_TIMEOUT_MS",
                "2000",
            )?),
            database_runtime_role: optional_role("DATABASE_RUNTIME_ROLE")?,
            database_identity_role: optional_role("DATABASE_IDENTITY_ROLE")?,
            redis_url: required("REDIS_URL")?,
            auth_jwt_issuer: required("AUTH_JWT_ISSUER")?,
            auth_jwt_audience: required("AUTH_JWT_AUDIENCE")?,
            auth_jwt_secret: SecretString::from(required("AUTH_JWT_SECRET")?),
            auth_jwt_lifetime_seconds: parse_or("AUTH_JWT_LIFETIME_SECONDS", "3600")?,
            mcp_allowed_hosts: comma_separated_or("MCP_ALLOWED_HOSTS", "localhost,127.0.0.1,::1")?,
            google_client_id: optional("GOOGLE_CLIENT_ID"),
            apple_client_id: optional("APPLE_CLIENT_ID"),
            storefront_public_base_url: "http://localhost:4321/".parse().unwrap(),
            resend_api_base_url: parse_or("RESEND_API_BASE_URL", "https://api.resend.com/")?,
            stripe_api_base_url: parse_or("STRIPE_API_BASE_URL", "https://api.stripe.com/")?,
            easypost_api_base_url: parse_or("EASYPOST_API_BASE_URL", "https://api.easypost.com/")?,
            analytics_meta_api_base_url: parse_or(
                "ANALYTICS_META_API_BASE_URL",
                "https://graph.facebook.com/v24.0/",
            )?,
            provider_secret_key: provider_secret_key()?,
            media_storage: media_storage_settings()?,
            shopper_token_active_key_id: required("SHOPPER_TOKEN_ACTIVE_KEY_ID")?,
            shopper_token_active_secret: required("SHOPPER_TOKEN_ACTIVE_SECRET")?,
            shopper_token_previous_key: optional_pair(
                "SHOPPER_TOKEN_PREVIOUS_KEY_ID",
                "SHOPPER_TOKEN_PREVIOUS_SECRET",
            )?,
            dependency_timeout: Duration::from_millis(parse_or("DEPENDENCY_TIMEOUT_MS", "1000")?),
            shutdown_drain_delay: Duration::from_millis(parse_or(
                "SHUTDOWN_DRAIN_DELAY_MS",
                "5000",
            )?),
            shutdown_worker_timeout: Duration::from_millis(parse_or(
                "SHUTDOWN_WORKER_TIMEOUT_MS",
                "30000",
            )?),
            log_filter: env::var("RUST_LOG")
                .unwrap_or_else(|_| "chaos=debug,chaos_api=debug,tower_http=info".into()),
            log_json: parse_or("LOG_JSON", "false")?,
        };
        if settings.dependency_timeout.is_zero()
            || settings.dependency_timeout > Duration::from_secs(10)
        {
            bail!("DEPENDENCY_TIMEOUT_MS must be between 1 and 10000");
        }
        Ok(settings)
    }
}

fn provider_secret_key() -> anyhow::Result<SecretKey> {
    let raw = required("CHAOS_PROVIDER_SECRET_KEY")?;
    SecretKey::from_base64(&raw)
        .with_context(|| "environment variable CHAOS_PROVIDER_SECRET_KEY is invalid")
}

fn media_storage_settings() -> anyhow::Result<Option<MediaStorageSettings>> {
    let names = [
        "MEDIA_S3_REGION",
        "MEDIA_S3_BUCKET",
        "MEDIA_S3_ACCESS_KEY_ID",
        "MEDIA_S3_SECRET_ACCESS_KEY",
        "MEDIA_PUBLIC_BASE_URL",
    ];
    let values = names.map(optional);
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    if values.iter().any(Option::is_none) {
        bail!(
            "Media storage requires MEDIA_S3_REGION, MEDIA_S3_BUCKET, MEDIA_S3_ACCESS_KEY_ID, MEDIA_S3_SECRET_ACCESS_KEY, and MEDIA_PUBLIC_BASE_URL together"
        );
    }
    Ok(Some(MediaStorageSettings {
        endpoint_url: optional("MEDIA_S3_ENDPOINT_URL"),
        region: values[0].clone().unwrap(),
        bucket: values[1].clone().unwrap(),
        access_key_id: values[2].clone().unwrap(),
        secret_access_key: values[3].clone().unwrap(),
        session_token: optional("MEDIA_S3_SESSION_TOKEN"),
        force_path_style: parse_or("MEDIA_S3_FORCE_PATH_STYLE", "false")?,
        public_base_url: values[4]
            .as_deref()
            .unwrap()
            .parse()
            .with_context(|| "environment variable MEDIA_PUBLIC_BASE_URL has an invalid value")?,
    }))
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn optional_pair(first: &str, second: &str) -> anyhow::Result<Option<(String, String)>> {
    match (env::var(first).ok(), env::var(second).ok()) {
        (None, None) => Ok(None),
        (Some(first_value), Some(second_value))
            if !first_value.trim().is_empty() && !second_value.trim().is_empty() =>
        {
            Ok(Some((first_value, second_value)))
        }
        _ => bail!("environment variables {first} and {second} must be set together"),
    }
}

fn optional_role(name: &str) -> anyhow::Result<Option<String>> {
    let Ok(value) = env::var(name) else {
        return Ok(None);
    };
    let value = value.trim();
    let mut bytes = value.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid {
        bail!("environment variable {name} must be a safe lowercase PostgreSQL role name");
    }
    Ok(Some(value.to_owned()))
}

fn required(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!("required environment variable {name} is not set"),
    }
}

fn comma_separated_or(name: &str, default: &str) -> anyhow::Result<Vec<String>> {
    let values = env::var(name).unwrap_or_else(|_| default.to_owned());
    let values = values
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty() {
        bail!("environment variable {name} must contain at least one host");
    }
    if values.iter().any(|value| {
        value.contains('/') || value.contains("//") || value.chars().any(char::is_whitespace)
    }) {
        bail!("environment variable {name} must contain comma-separated host authorities");
    }
    Ok(values)
}

fn parse_or<T>(name: &str, default: &str) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    env::var(name)
        .unwrap_or_else(|_| default.to_owned())
        .parse()
        .with_context(|| format!("environment variable {name} has an invalid value"))
}
