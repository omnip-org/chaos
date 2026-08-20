use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        AccessTokenCodec, AccessTokenGrant, ExternalIdentityVerifier, GeneratedMcpKeyMaterial,
        IdentityRepository, McpKeyListItem, McpKeyMaterialGenerator, McpKeyRepository,
        McpPrincipal, VerifiedExternalIdentity,
    },
};
use chaos_domain::identity::{Email, ExternalSubject, IdentityProvider, McpKey, McpKeyId, UserId};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
    jwk::JwkSet,
};
use rand::Rng;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use url::Url;
use uuid::Uuid;

const JWKS_CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);
const MCP_KEY_PREFIX: &str = "cc_mcp_v1_";

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
             FROM identity.external_identities AS external_identity \
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
            "INSERT INTO identity.external_identities \
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

#[derive(Default)]
pub struct SecureMcpKeyMaterialGenerator;

impl McpKeyMaterialGenerator for SecureMcpKeyMaterialGenerator {
    fn generate(&self) -> GeneratedMcpKeyMaterial {
        let mut identifier_bytes = [0_u8; 12];
        let mut secret_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut identifier_bytes);
        rand::rng().fill_bytes(&mut secret_bytes);
        let key_identifier = URL_SAFE_NO_PAD.encode(identifier_bytes);
        let secret = URL_SAFE_NO_PAD.encode(secret_bytes);
        let plaintext = format!("{MCP_KEY_PREFIX}{key_identifier}_{secret}");
        let secret_digest = Sha256::digest(plaintext.as_bytes()).into();
        let display_suffix = secret[secret.len() - 4..].to_owned();
        GeneratedMcpKeyMaterial {
            key_identifier,
            secret_digest,
            display_suffix,
            plaintext: SecretString::from(plaintext),
        }
    }
}

#[derive(Clone)]
pub struct PostgresMcpKeyRepository {
    pool: PgPool,
}

impl PostgresMcpKeyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl McpKeyRepository for PostgresMcpKeyRepository {
    async fn create(
        &self,
        key: &McpKey,
        material: &GeneratedMcpKeyMaterial,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO identity.mcp_keys \
             (id, user_id, key_identifier, secret_digest, display_suffix, name) \
             SELECT $1, identity_user.id, $2, $3, $4, $5 \
             FROM identity.users AS identity_user \
             WHERE identity_user.id = $6 AND identity_user.status = 'active'",
        )
        .bind(key.id().as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(key.name())
        .bind(key.user_id().as_uuid())
        .execute(&self.pool)
        .await
        .map_err(database_error)
        .and_then(|result| {
            if result.rows_affected() == 1 {
                Ok(())
            } else {
                Err(ApplicationError::Unauthorized)
            }
        })
    }

    async fn list(
        &self,
        user_id: UserId,
        after: Option<McpKeyId>,
        limit: u16,
    ) -> Result<Vec<McpKeyListItem>, ApplicationError> {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                OffsetDateTime,
                Option<OffsetDateTime>,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT id, name, key_identifier, display_suffix::text, created_at, \
                    last_used_at, revoked_at \
             FROM identity.mcp_keys \
             WHERE user_id = $1 AND ($2::uuid IS NULL OR id > $2) \
             ORDER BY id ASC \
             LIMIT $3",
        )
        .bind(user_id.as_uuid())
        .bind(after.map(McpKeyId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    name,
                    key_identifier,
                    display_suffix,
                    created_at,
                    last_used_at,
                    revoked_at,
                )| {
                    McpKeyListItem {
                        id: McpKeyId::from_uuid(id),
                        name,
                        key_identifier,
                        display_suffix,
                        created_at,
                        last_used_at,
                        revoked_at,
                    }
                },
            )
            .collect())
    }

    async fn revoke(&self, user_id: UserId, key_id: McpKeyId) -> Result<(), ApplicationError> {
        let result = sqlx::query(
            "UPDATE identity.mcp_keys \
             SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP), \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE id = $1 AND user_id = $2",
        )
        .bind(key_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(ApplicationError::NotFound {
                resource: "MCP key",
                id: key_id.as_uuid().to_string(),
            });
        }
        Ok(())
    }

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<McpPrincipal>, ApplicationError> {
        let Some(key_identifier) = parse_mcp_key_identifier(presented_key.expose_secret()) else {
            return Ok(None);
        };
        let digest: [u8; 32] = Sha256::digest(presented_key.expose_secret().as_bytes()).into();
        let row = sqlx::query_as::<_, (Uuid, Uuid)>(
            "WITH authenticated AS MATERIALIZED ( \
                 SELECT mcp_key.id, mcp_key.user_id \
                 FROM identity.mcp_keys AS mcp_key \
                 INNER JOIN identity.users AS identity_user \
                    ON identity_user.id = mcp_key.user_id \
                 WHERE mcp_key.key_identifier = $1 \
                   AND mcp_key.secret_digest = $2 \
                   AND mcp_key.revoked_at IS NULL \
                   AND (mcp_key.expires_at IS NULL \
                        OR mcp_key.expires_at > CURRENT_TIMESTAMP) \
                   AND identity_user.status = 'active' \
             ), touched AS ( \
                 UPDATE identity.mcp_keys AS mcp_key \
                 SET last_used_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP \
                 FROM authenticated \
                 WHERE mcp_key.id = authenticated.id \
                   AND (mcp_key.last_used_at IS NULL \
                        OR mcp_key.last_used_at < CURRENT_TIMESTAMP - INTERVAL '5 minutes') \
                 RETURNING mcp_key.id \
             ) \
             SELECT id, user_id FROM authenticated",
        )
        .bind(key_identifier)
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(row.map(|(key_id, user_id)| McpPrincipal {
            key_id: McpKeyId::from_uuid(key_id),
            user_id: UserId::from_uuid(user_id),
        }))
    }
}

fn parse_mcp_key_identifier(presented_key: &str) -> Option<&str> {
    let remainder = presented_key.strip_prefix(MCP_KEY_PREFIX)?;
    let (identifier, remainder) = remainder.split_at_checked(16)?;
    let secret = remainder.strip_prefix('_')?;
    (!secret.is_empty()).then_some(identifier)
}

#[derive(Clone)]
pub struct JwtAccessTokenCodec {
    issuer: String,
    audience: String,
    lifetime_seconds: u32,
    encoding_key: Arc<EncodingKey>,
    decoding_key: Arc<DecodingKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AccessTokenClaims {
    iss: String,
    aud: String,
    sub: String,
    iat: i64,
    exp: i64,
}

impl JwtAccessTokenCodec {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        secret: &[u8],
        lifetime_seconds: u32,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            secret.len() >= 32,
            "AUTH_JWT_SECRET must contain at least 32 bytes"
        );
        anyhow::ensure!(
            lifetime_seconds > 0,
            "AUTH_JWT_LIFETIME_SECONDS must be positive"
        );
        let issuer = issuer.into();
        let audience = audience.into();
        anyhow::ensure!(
            !issuer.trim().is_empty(),
            "AUTH_JWT_ISSUER must not be empty"
        );
        anyhow::ensure!(
            !audience.trim().is_empty(),
            "AUTH_JWT_AUDIENCE must not be empty"
        );
        Ok(Self {
            issuer,
            audience,
            lifetime_seconds,
            encoding_key: Arc::new(EncodingKey::from_secret(secret)),
            decoding_key: Arc::new(DecodingKey::from_secret(secret)),
        })
    }
}

impl AccessTokenCodec for JwtAccessTokenCodec {
    fn issue(&self, user_id: UserId) -> Result<AccessTokenGrant, ApplicationError> {
        let issued_at = OffsetDateTime::now_utc().unix_timestamp();
        let claims = AccessTokenClaims {
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            sub: user_id.as_uuid().to_string(),
            iat: issued_at,
            exp: issued_at + i64::from(self.lifetime_seconds),
        };
        let token = encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        Ok(AccessTokenGrant {
            user_id,
            token: SecretString::from(token),
            expires_in_seconds: self.lifetime_seconds,
        })
    }

    fn verify(&self, token: &SecretString) -> Result<UserId, ApplicationError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
        validation.leeway = 30;
        let claims =
            decode::<AccessTokenClaims>(token.expose_secret(), &self.decoding_key, &validation)
                .map_err(|_| ApplicationError::Unauthorized)?
                .claims;
        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApplicationError::Unauthorized)?;
        Ok(UserId::from_uuid(user_id))
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_mcp_key_has_a_versioned_user_credential_shape() {
        let generated = SecureMcpKeyMaterialGenerator.generate();
        let plaintext = generated.plaintext.expose_secret();
        assert!(plaintext.starts_with(MCP_KEY_PREFIX));
        assert_eq!(
            parse_mcp_key_identifier(plaintext),
            Some(generated.key_identifier.as_str())
        );
        assert_eq!(
            generated.secret_digest,
            <[u8; 32]>::from(Sha256::digest(plaintext.as_bytes()))
        );
        assert!(plaintext.ends_with(&generated.display_suffix));
    }

    #[test]
    fn jwt_round_trip_preserves_user_identity() {
        let codec = JwtAccessTokenCodec::new(
            "https://identity.chaos.test",
            "chaos-api",
            b"a-test-secret-that-is-at-least-32-bytes-long",
            900,
        )
        .unwrap();
        let user_id = UserId::new();
        let grant = codec.issue(user_id).unwrap();
        assert_eq!(codec.verify(&grant.token).unwrap(), user_id);
        assert_eq!(grant.expires_in_seconds, 900);
    }

    #[test]
    fn jwt_rejects_a_different_signing_key() {
        let issuer_codec = JwtAccessTokenCodec::new(
            "https://identity.chaos.test",
            "chaos-api",
            b"first-test-secret-that-is-at-least-32-bytes",
            900,
        )
        .unwrap();
        let verifier_codec = JwtAccessTokenCodec::new(
            "https://identity.chaos.test",
            "chaos-api",
            b"second-test-secret-that-is-at-least-32-bytes",
            900,
        )
        .unwrap();
        let grant = issuer_codec.issue(UserId::new()).unwrap();
        assert!(matches!(
            verifier_codec.verify(&grant.token),
            Err(ApplicationError::Unauthorized)
        ));
    }
}
