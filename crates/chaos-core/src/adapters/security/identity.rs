use std::{collections::HashMap, time::Duration};

use crate::{
    ApplicationError,
    contracts::{ExternalIdentityVerifier, IdentityRepository, VerifiedExternalIdentity},
    error::database_error,
};
use async_trait::async_trait;
use chaos_domain::identity::{Email, ExternalSubject, IdentityProvider, UserId};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::PgPool;
use tokio::sync::RwLock;
use url::Url;
use uuid::Uuid;

const JWKS_CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);
#[derive(Clone, Debug)]
pub struct OidcProviderConfiguration {
    pub provider: IdentityProvider,
    pub issuers: Vec<String>,
    pub audience: String,
    pub jwks_uri: Url,
}

#[derive(Clone)]
struct CachedJwks {
    fetched_at: std::time::Instant,
    keys: JwkSet,
}

pub struct OidcIdentityVerifier {
    client: Client,
    providers: HashMap<IdentityProvider, OidcProviderConfiguration>,
    cache: RwLock<HashMap<IdentityProvider, CachedJwks>>,
}

impl OidcIdentityVerifier {
    pub fn new(
        configurations: impl IntoIterator<Item = OidcProviderConfiguration>,
        request_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let providers: HashMap<_, _> = configurations
            .into_iter()
            .map(|configuration| (configuration.provider, configuration))
            .collect();
        anyhow::ensure!(
            !providers.is_empty(),
            "at least one external identity provider must be configured"
        );
        for configuration in providers.values() {
            anyhow::ensure!(
                !configuration.audience.trim().is_empty(),
                "identity provider audience must not be empty"
            );
            anyhow::ensure!(
                !configuration.issuers.is_empty()
                    && configuration
                        .issuers
                        .iter()
                        .all(|issuer| !issuer.trim().is_empty()),
                "identity provider issuers must not be empty"
            );
            anyhow::ensure!(
                configuration.jwks_uri.scheme() == "https",
                "identity provider JWKS URI must use HTTPS"
            );
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|error| anyhow::anyhow!("failed to build OIDC HTTP client: {error}"))?;
        Ok(Self {
            client,
            providers,
            cache: RwLock::new(HashMap::new()),
        })
    }

    async fn jwks(
        &self,
        configuration: &OidcProviderConfiguration,
        force_refresh: bool,
    ) -> Result<JwkSet, ApplicationError> {
        if !force_refresh {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&configuration.provider)
                && cached.fetched_at.elapsed() < JWKS_CACHE_LIFETIME
            {
                return Ok(cached.keys.clone());
            }
        }
        let keys = self
            .client
            .get(configuration.jwks_uri.clone())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| ApplicationError::Unavailable {
                service: "identity_provider",
                source: error.into(),
            })?
            .json::<JwkSet>()
            .await
            .map_err(|error| ApplicationError::Unavailable {
                service: "identity_provider",
                source: error.into(),
            })?;
        self.cache.write().await.insert(
            configuration.provider,
            CachedJwks {
                fetched_at: std::time::Instant::now(),
                keys: keys.clone(),
            },
        );
        Ok(keys)
    }
}

#[derive(Deserialize)]
struct OidcClaims {
    sub: String,
    email: String,
    email_verified: serde_json::Value,
}

#[async_trait]
impl ExternalIdentityVerifier for OidcIdentityVerifier {
    async fn verify(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<VerifiedExternalIdentity, ApplicationError> {
        let configuration = self
            .providers
            .get(&provider)
            .ok_or(ApplicationError::Unauthorized)?;
        let header = decode_header(identity_token.expose_secret())
            .map_err(|_| ApplicationError::Unauthorized)?;
        if header.alg != Algorithm::RS256 {
            return Err(ApplicationError::Unauthorized);
        }
        let key_id = header.kid.ok_or(ApplicationError::Unauthorized)?;
        let mut keys = self.jwks(configuration, false).await?;
        let key = match keys.find(&key_id) {
            Some(key) => key.clone(),
            None => {
                keys = self.jwks(configuration, true).await?;
                keys.find(&key_id)
                    .cloned()
                    .ok_or(ApplicationError::Unauthorized)?
            }
        };
        let decoding_key =
            DecodingKey::from_jwk(&key).map_err(|_| ApplicationError::Unauthorized)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&configuration.audience]);
        validation.set_issuer(&configuration.issuers);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
        validation.leeway = 30;
        let claims =
            decode::<OidcClaims>(identity_token.expose_secret(), &decoding_key, &validation)
                .map_err(|_| ApplicationError::Unauthorized)?
                .claims;
        let email_verified = claims.email_verified == serde_json::Value::Bool(true)
            || claims.email_verified == serde_json::Value::String("true".into());
        if !email_verified {
            return Err(ApplicationError::Unauthorized);
        }
        Ok(VerifiedExternalIdentity {
            provider,
            subject: ExternalSubject::parse(claims.sub).map_err(ApplicationError::from)?,
            email: Email::parse(claims.email).map_err(ApplicationError::from)?,
        })
    }
}

pub struct PostgresIdentityRepository {
    pool: PgPool,
}

impl PostgresIdentityRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IdentityRepository for PostgresIdentityRepository {
    async fn resolve_user(
        &self,
        identity: &VerifiedExternalIdentity,
    ) -> Result<UserId, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "{}:{}",
                identity.provider.as_str(),
                identity.subject.as_str()
            ))
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let existing: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT external_identity.user_id, identity_user.status::TEXT \
             FROM identity.credentials AS external_identity \
             INNER JOIN identity.users AS identity_user \
                ON identity_user.id = external_identity.user_id \
             WHERE external_identity.provider = $1::identity.identity_provider \
               AND external_identity.subject = $2",
        )
        .bind(identity.provider.as_str())
        .bind(identity.subject.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        if let Some((user_id, status)) = existing {
            if status != "active" {
                return Err(ApplicationError::Unauthorized);
            }
            transaction.commit().await.map_err(database_error)?;
            return Ok(UserId::from_uuid(user_id));
        }

        let user_id = UserId::new();
        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(identity.email.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(identity_write_error)?;
        sqlx::query(
            "INSERT INTO identity.credentials \
             (provider, subject, user_id, email) \
             VALUES ($1::identity.identity_provider, $2, $3, $4)",
        )
        .bind(identity.provider.as_str())
        .bind(identity.subject.as_str())
        .bind(user_id.as_uuid())
        .bind(identity.email.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(identity_write_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(user_id)
    }
}

fn identity_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.code().as_deref() == Some("23505")
    {
        return ApplicationError::Conflict {
            code: "identity_link_required",
            message: "an account already uses this email; sign in to that account before linking another provider",
        };
    }
    database_error(error)
}
