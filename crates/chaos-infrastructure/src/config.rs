use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use anyhow::{Context, bail};

#[derive(Clone, Debug)]
pub struct Settings {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub database_control_plane_url: String,
    pub database_max_connections: u32,
    pub database_control_plane_max_connections: u32,
    pub database_acquire_timeout: Duration,
    pub database_runtime_role: Option<String>,
    pub database_control_plane_role: Option<String>,
    pub redis_url: String,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub auth_public_base_url: String,
    pub smtp_url: String,
    pub email_from: String,
    pub dependency_timeout: Duration,
    pub shutdown_drain_delay: Duration,
    pub log_filter: String,
    pub log_json: bool,
}

impl Settings {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            bind_addr: parse_or("APP_BIND_ADDR", "0.0.0.0:8080")?,
            database_url: required("DATABASE_URL")?,
            database_control_plane_url: required("DATABASE_CONTROL_PLANE_URL")?,
            database_max_connections: parse_or("DATABASE_MAX_CONNECTIONS", "20")?,
            database_control_plane_max_connections: parse_or(
                "DATABASE_CONTROL_PLANE_MAX_CONNECTIONS",
                "5",
            )?,
            database_acquire_timeout: Duration::from_millis(parse_or(
                "DATABASE_ACQUIRE_TIMEOUT_MS",
                "2000",
            )?),
            database_runtime_role: optional_role("DATABASE_RUNTIME_ROLE")?,
            database_control_plane_role: optional_role("DATABASE_CONTROL_PLANE_ROLE")?,
            redis_url: required("REDIS_URL")?,
            webauthn_rp_id: required("WEBAUTHN_RP_ID")?,
            webauthn_rp_origin: required("WEBAUTHN_RP_ORIGIN")?,
            auth_public_base_url: required("AUTH_PUBLIC_BASE_URL")?,
            smtp_url: required("SMTP_URL")?,
            email_from: required("EMAIL_FROM")?,
            dependency_timeout: Duration::from_millis(parse_or("DEPENDENCY_TIMEOUT_MS", "1000")?),
            shutdown_drain_delay: Duration::from_millis(parse_or(
                "SHUTDOWN_DRAIN_DELAY_MS",
                "5000",
            )?),
            log_filter: env::var("RUST_LOG")
                .unwrap_or_else(|_| "chaos=debug,chaos_api=debug,tower_http=info".into()),
            log_json: parse_or("LOG_JSON", "false")?,
        })
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
