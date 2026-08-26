use std::collections::BTreeSet;

use crate::{ApplicationError, error::database_error};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_domain::identity::UserId;
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

pub const MCP_SCOPE: &str = "mcp";
const ACCESS_TOKEN_PREFIX: &str = "mcp_at_";
const REFRESH_TOKEN_PREFIX: &str = "mcp_rt_";
const AUTHORIZATION_CODE_PREFIX: &str = "mcp_code_";
const AUTHORIZATION_REQUEST_LIFETIME: Duration = Duration::minutes(10);
const AUTHORIZATION_CODE_LIFETIME: Duration = Duration::minutes(5);
const ACCESS_TOKEN_LIFETIME: Duration = Duration::minutes(15);
const REFRESH_TOKEN_LIFETIME: Duration = Duration::days(30);

#[derive(Clone)]
pub struct McpOAuthService {
    pool: PgPool,
    issuer: String,
    resource: String,
    google_client_id: Option<String>,
    apple_client_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub application_type: String,
}

#[derive(Clone, Debug)]
pub struct AuthorizationPage {
    pub transaction_id: Uuid,
    pub client_name: String,
    pub scope: String,
}

#[derive(Clone, Debug)]
pub struct AuthorizationRedirect {
    pub location: String,
}

#[derive(Debug)]
pub struct OAuthTokenSet {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub expires_in_seconds: u32,
    pub scope: String,
}

#[derive(Clone, Debug)]
pub struct McpOAuthPrincipal {
    pub user_id: UserId,
    pub scope: String,
}

impl McpOAuthService {
    pub fn new(
        pool: PgPool,
        public_base_url: &Url,
        google_client_id: Option<String>,
        apple_client_id: Option<String>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(public_base_url.scheme(), "http" | "https")
                && public_base_url.host_str().is_some(),
            "PUBLIC_BASE_URL must be an absolute HTTP(S) URL"
        );
        anyhow::ensure!(
            public_base_url.query().is_none() && public_base_url.fragment().is_none(),
            "PUBLIC_BASE_URL must not contain a query or fragment"
        );
        anyhow::ensure!(
            public_base_url.path().is_empty() || public_base_url.path() == "/",
            "PUBLIC_BASE_URL must identify the API origin without a path"
        );
        let issuer = public_base_url.as_str().trim_end_matches('/').to_owned();
        let resource = format!("{issuer}/mcp/v1");
        Ok(Self {
            pool,
            issuer,
            resource,
            google_client_id,
            apple_client_id,
        })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }

    pub fn authorization_endpoint(&self) -> String {
        format!("{}/oauth/authorize", self.issuer)
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}/oauth/token", self.issuer)
    }

    pub fn registration_endpoint(&self) -> String {
        format!("{}/oauth/register", self.issuer)
    }

    pub fn protected_resource_metadata_endpoint(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource", self.issuer)
    }

    pub fn google_client_id(&self) -> Option<&str> {
        self.google_client_id.as_deref()
    }

    pub fn apple_client_id(&self) -> Option<&str> {
        self.apple_client_id.as_deref()
    }

    pub fn valid_resource(&self, resource: Option<&str>) -> bool {
        resource.is_some_and(|value| value == self.resource)
    }

    pub fn normalize_scope(scope: Option<&str>) -> Result<String, ApplicationError> {
        let mut scopes = BTreeSet::new();
        for value in scope.unwrap_or(MCP_SCOPE).split_whitespace() {
            if value != MCP_SCOPE {
                return Err(ApplicationError::Forbidden);
            }
            scopes.insert(value);
        }
        if scopes.is_empty() {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "scope",
                    reason: "must include the mcp scope".into(),
                }],
            });
        }
        Ok(scopes.into_iter().collect::<Vec<_>>().join(" "))
    }

    pub async fn register_client(
        &self,
        client_name: String,
        redirect_uris: Vec<String>,
        grant_types: Vec<String>,
        response_types: Vec<String>,
        application_type: String,
    ) -> Result<OAuthClient, ApplicationError> {
        let client = OAuthClient {
            client_id: format!("mcp_client_{}", Uuid::now_v7().simple()),
            client_name,
            redirect_uris,
            grant_types,
            response_types,
            token_endpoint_auth_method: "none".into(),
            application_type,
        };
        sqlx::query(
            "INSERT INTO identity.oauth_clients
             (client_id, client_name, redirect_uris, grant_types, response_types,
              token_endpoint_auth_method, application_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&client.client_id)
        .bind(&client.client_name)
        .bind(serde_json::to_value(&client.redirect_uris).expect("redirect URI serialization"))
        .bind(serde_json::to_value(&client.grant_types).expect("grant type serialization"))
        .bind(serde_json::to_value(&client.response_types).expect("response type serialization"))
        .bind(&client.token_endpoint_auth_method)
        .bind(&client.application_type)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(client)
    }

    pub async fn find_client(
        &self,
        client_id: &str,
    ) -> Result<Option<OAuthClient>, ApplicationError> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                serde_json::Value,
                serde_json::Value,
                serde_json::Value,
                String,
                String,
            ),
        >(
            "SELECT client_id, client_name, redirect_uris, grant_types, response_types,
                    token_endpoint_auth_method, application_type
             FROM identity.oauth_clients
             WHERE client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(parse_client).transpose()
    }

    pub async fn start_authorization(
        &self,
        client: &OAuthClient,
        redirect_uri: &str,
        scope: &str,
        state: Option<&str>,
        code_challenge: &str,
        code_challenge_method: &str,
    ) -> Result<AuthorizationPage, ApplicationError> {
        let transaction_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO identity.oauth_authorization_requests
             (id, client_id, redirect_uri, scope, state, code_challenge,
              code_challenge_method, resource, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(transaction_id)
        .bind(&client.client_id)
        .bind(redirect_uri)
        .bind(scope)
        .bind(state)
        .bind(code_challenge)
        .bind(code_challenge_method)
        .bind(&self.resource)
        .bind(OffsetDateTime::now_utc() + AUTHORIZATION_REQUEST_LIFETIME)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(AuthorizationPage {
            transaction_id,
            client_name: client.client_name.clone(),
            scope: scope.to_owned(),
        })
    }

    pub async fn finish_authorization(
        &self,
        transaction_id: Uuid,
        user_id: UserId,
    ) -> Result<AuthorizationRedirect, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                String,
                String,
                String,
                OffsetDateTime,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT client_id, redirect_uri, scope, state, code_challenge,
                    code_challenge_method, resource, expires_at, used_at
             FROM identity.oauth_authorization_requests
             WHERE id = $1
             FOR UPDATE",
        )
        .bind(transaction_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        let (
            client_id,
            redirect_uri,
            scope,
            state,
            code_challenge,
            code_challenge_method,
            resource,
            expires_at,
            used_at,
        ) = row;
        if used_at.is_some() || expires_at <= OffsetDateTime::now_utc() {
            return Err(ApplicationError::Unauthorized);
        }
        if resource != self.resource {
            return Err(ApplicationError::Unauthorized);
        }

        let code = random_token(AUTHORIZATION_CODE_PREFIX);
        let code_digest = digest(&code);
        sqlx::query(
            "UPDATE identity.oauth_authorization_requests
             SET used_at = CURRENT_TIMESTAMP
             WHERE id = $1",
        )
        .bind(transaction_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO identity.oauth_authorization_codes
             (code_digest, client_id, user_id, redirect_uri, scope, code_challenge,
              code_challenge_method, resource, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(code_digest.as_slice())
        .bind(&client_id)
        .bind(user_id.as_uuid())
        .bind(&redirect_uri)
        .bind(&scope)
        .bind(&code_challenge)
        .bind(&code_challenge_method)
        .bind(&resource)
        .bind(OffsetDateTime::now_utc() + AUTHORIZATION_CODE_LIFETIME)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;

        let mut location = Url::parse(&redirect_uri).map_err(|error| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "stored OAuth redirect URI is invalid: {error}"
            ))
        })?;
        {
            let mut query = location.query_pairs_mut();
            query.append_pair("code", &code);
            if let Some(state) = state {
                query.append_pair("state", &state);
            }
            query.append_pair("iss", &self.issuer);
        }
        Ok(AuthorizationRedirect {
            location: location.into(),
        })
    }

    pub async fn redeem_authorization_code(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenSet, ApplicationError> {
        if !valid_code_verifier(code_verifier) {
            return Err(ApplicationError::Unauthorized);
        }
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let row = sqlx::query_as::<
            _,
            (
                String,
                Uuid,
                String,
                String,
                String,
                String,
                String,
                OffsetDateTime,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT code.client_id, code.user_id, code.redirect_uri, code.scope,
                    code.code_challenge, code.code_challenge_method, code.resource,
                    code.expires_at, code.consumed_at
             FROM identity.oauth_authorization_codes AS code
             INNER JOIN identity.users AS identity_user
                ON identity_user.id = code.user_id AND identity_user.status = 'active'
             WHERE code.code_digest = $1
             FOR UPDATE",
        )
        .bind(digest(code).as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        let (
            stored_client_id,
            user_id,
            stored_redirect_uri,
            scope,
            code_challenge,
            code_challenge_method,
            resource,
            expires_at,
            consumed_at,
        ) = row;
        if consumed_at.is_some()
            || expires_at <= OffsetDateTime::now_utc()
            || stored_client_id != client_id
            || stored_redirect_uri != redirect_uri
            || resource != self.resource
            || code_challenge_method != "S256"
            || !verify_pkce(code_verifier, &code_challenge)
        {
            return Err(ApplicationError::Unauthorized);
        }
        sqlx::query(
            "UPDATE identity.oauth_authorization_codes
             SET consumed_at = CURRENT_TIMESTAMP
             WHERE code_digest = $1",
        )
        .bind(digest(code).as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let tokens = issue_tokens(&mut transaction, client_id, user_id, &scope, &resource).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(tokens)
    }

    pub async fn rotate_refresh_token(
        &self,
        client_id: &str,
        refresh_token: &str,
        resource: &str,
    ) -> Result<OAuthTokenSet, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let old_digest = digest(refresh_token);
        let row = sqlx::query_as::<
            _,
            (
                String,
                Uuid,
                String,
                String,
                OffsetDateTime,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT token.client_id, token.user_id, token.scope, token.resource,
                    token.expires_at, token.revoked_at
             FROM identity.oauth_refresh_tokens AS token
             INNER JOIN identity.users AS identity_user
                ON identity_user.id = token.user_id AND identity_user.status = 'active'
             WHERE token.token_digest = $1
             FOR UPDATE",
        )
        .bind(old_digest.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        let (stored_client_id, user_id, scope, stored_resource, expires_at, revoked_at) = row;
        if revoked_at.is_some() {
            // Do not let an invalid refresh request revoke another client's
            // token family. The token row is locked, but the client and
            // resource binding still must be checked before mass revocation.
            if stored_client_id != client_id
                || stored_resource != resource
                || resource != self.resource
            {
                transaction.rollback().await.map_err(database_error)?;
                return Err(ApplicationError::Unauthorized);
            }
            // A rotated refresh token being replayed is a signal that the
            // token family may have been copied. Revoke the remaining family
            // before returning the generic invalid_grant response.
            sqlx::query(
                "UPDATE identity.oauth_refresh_tokens
                 SET revoked_at = CURRENT_TIMESTAMP
                 WHERE client_id = $1 AND user_id = $2 AND revoked_at IS NULL",
            )
            .bind(&stored_client_id)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "UPDATE identity.oauth_access_tokens
                 SET revoked_at = CURRENT_TIMESTAMP
                 WHERE client_id = $1 AND user_id = $2 AND revoked_at IS NULL",
            )
            .bind(&stored_client_id)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            transaction.commit().await.map_err(database_error)?;
            return Err(ApplicationError::Unauthorized);
        }
        if expires_at <= OffsetDateTime::now_utc()
            || stored_client_id != client_id
            || stored_resource != resource
            || resource != self.resource
        {
            return Err(ApplicationError::Unauthorized);
        }
        let tokens = issue_tokens(&mut transaction, client_id, user_id, &scope, resource).await?;
        let new_digest = digest(tokens.refresh_token.expose_secret());
        sqlx::query(
            "UPDATE identity.oauth_refresh_tokens
             SET revoked_at = CURRENT_TIMESTAMP, replaced_by_digest = $2
             WHERE token_digest = $1",
        )
        .bind(old_digest.as_slice())
        .bind(new_digest.as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(tokens)
    }

    pub async fn authenticate_access_token(
        &self,
        token: &SecretString,
    ) -> Result<McpOAuthPrincipal, ApplicationError> {
        if !token.expose_secret().starts_with(ACCESS_TOKEN_PREFIX) {
            return Err(ApplicationError::Unauthorized);
        }
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT token.user_id, token.scope
             FROM identity.oauth_access_tokens AS token
             INNER JOIN identity.users AS identity_user
                ON identity_user.id = token.user_id AND identity_user.status = 'active'
             WHERE token.token_digest = $1
               AND token.resource = $2
               AND token.revoked_at IS NULL
               AND token.expires_at > CURRENT_TIMESTAMP",
        )
        .bind(digest(token.expose_secret()).as_slice())
        .bind(&self.resource)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        if !has_scope(&row.1, MCP_SCOPE) {
            return Err(ApplicationError::Forbidden);
        }
        Ok(McpOAuthPrincipal {
            user_id: UserId::from_uuid(row.0),
            scope: row.1,
        })
    }
}

async fn issue_tokens(
    transaction: &mut Transaction<'_, Postgres>,
    client_id: &str,
    user_id: Uuid,
    scope: &str,
    resource: &str,
) -> Result<OAuthTokenSet, ApplicationError> {
    let access_token = random_token(ACCESS_TOKEN_PREFIX);
    let refresh_token = random_token(REFRESH_TOKEN_PREFIX);
    let access_expires_at = OffsetDateTime::now_utc() + ACCESS_TOKEN_LIFETIME;
    let refresh_expires_at = OffsetDateTime::now_utc() + REFRESH_TOKEN_LIFETIME;
    sqlx::query(
        "INSERT INTO identity.oauth_access_tokens
         (token_digest, client_id, user_id, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(digest(&access_token).as_slice())
    .bind(client_id)
    .bind(user_id)
    .bind(scope)
    .bind(resource)
    .bind(access_expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO identity.oauth_refresh_tokens
         (token_digest, client_id, user_id, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(digest(&refresh_token).as_slice())
    .bind(client_id)
    .bind(user_id)
    .bind(scope)
    .bind(resource)
    .bind(refresh_expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(OAuthTokenSet {
        access_token: SecretString::from(access_token),
        refresh_token: SecretString::from(refresh_token),
        expires_in_seconds: ACCESS_TOKEN_LIFETIME.whole_seconds() as u32,
        scope: scope.to_owned(),
    })
}

fn parse_client(
    (
        client_id,
        client_name,
        redirect_uris,
        grant_types,
        response_types,
        token_endpoint_auth_method,
        application_type,
    ): (
        String,
        String,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        String,
        String,
    ),
) -> Result<OAuthClient, ApplicationError> {
    let parse = |value: serde_json::Value, field: &'static str| {
        serde_json::from_value::<Vec<String>>(value).map_err(|error| {
            ApplicationError::Unexpected(anyhow::anyhow!("invalid OAuth client {field}: {error}"))
        })
    };
    Ok(OAuthClient {
        client_id,
        client_name,
        redirect_uris: parse(redirect_uris, "redirect_uris")?,
        grant_types: parse(grant_types, "grant_types")?,
        response_types: parse(response_types, "response_types")?,
        token_endpoint_auth_method,
        application_type,
    })
}

fn random_token(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn valid_code_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
}

fn verify_pkce(verifier: &str, challenge: &str) -> bool {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
}

fn has_scope(scope: &str, required: &str) -> bool {
    scope.split_whitespace().any(|value| value == required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_uses_s256_and_rejects_weak_verifiers() {
        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(valid_code_verifier(verifier));
        assert!(verify_pkce(verifier, &challenge));
        assert!(!valid_code_verifier("too-short"));
        assert!(!verify_pkce("wrong-verifier", &challenge));
    }

    #[test]
    fn scope_is_restricted_to_the_mcp_resource_scope() {
        assert_eq!(McpOAuthService::normalize_scope(None).unwrap(), MCP_SCOPE);
        assert_eq!(
            McpOAuthService::normalize_scope(Some("mcp mcp")).unwrap(),
            MCP_SCOPE
        );
        assert!(McpOAuthService::normalize_scope(Some("admin")).is_err());
    }
}
