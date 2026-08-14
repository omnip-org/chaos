use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        CeremonyOptions as ApplicationCeremonyOptions, PasswordlessAuthentication, SessionGrant,
    },
};
use chaos_domain::identity::{Email, UserId};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
};
use rand::RngCore;
use redis::{AsyncCommands, Client as RedisClient};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use url::Url;
use uuid::Uuid;
use webauthn_rs::{
    Webauthn, WebauthnBuilder,
    prelude::{
        CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration,
        PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse,
    },
};

const MAGIC_LINK_LIFETIME_SECONDS: i32 = 15 * 60;
const SESSION_LIFETIME_SECONDS: i32 = 30 * 24 * 60 * 60;
const WEBAUTHN_CEREMONY_LIFETIME_SECONDS: u64 = 5 * 60;
const MAGIC_LINK_REQUEST_LIMIT: i64 = 3;
const MAGIC_LINK_REQUEST_WINDOW_SECONDS: i64 = 15 * 60;
const PASSKEY_AUTHENTICATION_LIMIT: i64 = 10;
const PASSKEY_AUTHENTICATION_WINDOW_SECONDS: i64 = 5 * 60;

#[derive(Clone)]
pub struct PasswordlessAuth {
    postgres: PgPool,
    redis: RedisClient,
    webauthn: Webauthn,
    mailer: AsyncSmtpTransport<Tokio1Executor>,
    email_from: Mailbox,
    auth_public_base_url: String,
}

struct CeremonyOptions<T> {
    pub ceremony_id: Uuid,
    pub public_key: T,
}

#[async_trait]
impl PasswordlessAuthentication for PasswordlessAuth {
    async fn request_magic_link(&self, email: Email) -> Result<(), ApplicationError> {
        PasswordlessAuth::request_magic_link(self, email).await
    }

    async fn consume_magic_link(
        &self,
        token: &SecretString,
    ) -> Result<SessionGrant, ApplicationError> {
        PasswordlessAuth::consume_magic_link(self, token).await
    }

    async fn authenticate_session(&self, token: &SecretString) -> Result<UserId, ApplicationError> {
        PasswordlessAuth::authenticate_session(self, token).await
    }

    async fn revoke_session(&self, token: &SecretString) -> Result<(), ApplicationError> {
        PasswordlessAuth::revoke_session(self, token).await
    }

    async fn start_passkey_registration(
        &self,
        user_id: UserId,
    ) -> Result<ApplicationCeremonyOptions, ApplicationError> {
        let options = PasswordlessAuth::start_passkey_registration(self, user_id).await?;
        Ok(ApplicationCeremonyOptions {
            ceremony_id: options.ceremony_id,
            public_key: serde_json::to_value(options.public_key)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        })
    }

    async fn finish_passkey_registration(
        &self,
        user_id: UserId,
        ceremony_id: Uuid,
        credential: serde_json::Value,
        name: &str,
    ) -> Result<Uuid, ApplicationError> {
        let credential = serde_json::from_value(credential)
            .map_err(|_| invalid_webauthn_credential_placeholder())?;
        PasswordlessAuth::finish_passkey_registration(self, user_id, ceremony_id, credential, name)
            .await
    }

    async fn start_passkey_authentication(
        &self,
        email: Email,
    ) -> Result<ApplicationCeremonyOptions, ApplicationError> {
        let options = PasswordlessAuth::start_passkey_authentication(self, email).await?;
        Ok(ApplicationCeremonyOptions {
            ceremony_id: options.ceremony_id,
            public_key: serde_json::to_value(options.public_key)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        })
    }

    async fn finish_passkey_authentication(
        &self,
        ceremony_id: Uuid,
        credential: serde_json::Value,
    ) -> Result<SessionGrant, ApplicationError> {
        let credential =
            serde_json::from_value(credential).map_err(|_| ApplicationError::Unauthorized)?;
        PasswordlessAuth::finish_passkey_authentication(self, ceremony_id, credential).await
    }
}

#[derive(Serialize, Deserialize)]
struct RegistrationCeremony {
    user_id: Uuid,
    state: PasskeyRegistration,
}

#[derive(Serialize, Deserialize)]
struct AuthenticationCeremony {
    user_id: Uuid,
    state: PasskeyAuthentication,
}

impl PasswordlessAuth {
    pub fn new(
        postgres: PgPool,
        redis: RedisClient,
        rp_id: &str,
        rp_origin: &str,
        smtp_url: &str,
        email_from: &str,
        auth_public_base_url: &str,
    ) -> anyhow::Result<Self> {
        let origin = Url::parse(rp_origin)
            .map_err(|error| anyhow::anyhow!("invalid WEBAUTHN_RP_ORIGIN: {error}"))?;
        anyhow::ensure!(
            origin.scheme() == "https" || origin.host_str() == Some("localhost"),
            "WEBAUTHN_RP_ORIGIN must use HTTPS except on localhost"
        );
        let webauthn = WebauthnBuilder::new(rp_id, &origin)
            .map_err(|error| anyhow::anyhow!("invalid WebAuthn configuration: {error}"))?
            .build()
            .map_err(|error| anyhow::anyhow!("invalid WebAuthn configuration: {error}"))?;
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::from_url(smtp_url)
            .map_err(|error| anyhow::anyhow!("invalid SMTP_URL: {error}"))?
            .build();
        let email_from = email_from
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid EMAIL_FROM: {error}"))?;
        let auth_public_base_url = auth_public_base_url.trim_end_matches('/').to_owned();

        Ok(Self {
            postgres,
            redis,
            webauthn,
            mailer,
            email_from,
            auth_public_base_url,
        })
    }

    pub async fn request_magic_link(&self, email: Email) -> Result<(), ApplicationError> {
        if !self
            .consume_rate_limit(
                "magic-link",
                email.as_str(),
                MAGIC_LINK_REQUEST_LIMIT,
                MAGIC_LINK_REQUEST_WINDOW_SECONDS,
            )
            .await?
        {
            return Ok(());
        }
        let token = random_token();
        let token_digest = token_digest(token.expose_secret());

        sqlx::query(
            "INSERT INTO identity.magic_link_challenges \
             (id, email, token_digest, expires_at) \
             VALUES ($1, $2, $3, CURRENT_TIMESTAMP + make_interval(secs => $4))",
        )
        .bind(Uuid::now_v7())
        .bind(email.as_str())
        .bind(token_digest.as_slice())
        .bind(MAGIC_LINK_LIFETIME_SECONDS)
        .execute(&self.postgres)
        .await
        .map_err(database_error)?;
        let encoded_token = token.expose_secret();
        let sign_in_url = format!(
            "{}/auth/email-link#token={encoded_token}",
            self.auth_public_base_url
        );
        let message = Message::builder()
            .from(self.email_from.clone())
            .to(email
                .as_str()
                .parse::<Mailbox>()
                .map_err(|error| ApplicationError::Unexpected(anyhow::anyhow!(error)))?)
            .subject("Sign in to Chaos")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "Use this link to sign in:\n\n{sign_in_url}\n\nThis link expires in 15 minutes and can be used once."
            ))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;

        self.mailer
            .send(message)
            .await
            .map_err(|error| ApplicationError::Unavailable {
                service: "email",
                source: error.into(),
            })?;
        Ok(())
    }

    pub async fn consume_magic_link(
        &self,
        token: &SecretString,
    ) -> Result<SessionGrant, ApplicationError> {
        let digest = decode_and_digest_token(token)?;
        let mut transaction = self.postgres.begin().await.map_err(database_error)?;
        let email = sqlx::query_scalar::<_, String>(
            "UPDATE identity.magic_link_challenges \
             SET consumed_at = CURRENT_TIMESTAMP \
             WHERE token_digest = $1 \
               AND consumed_at IS NULL \
               AND expires_at > CURRENT_TIMESTAMP \
             RETURNING email::text",
        )
        .bind(digest.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;

        let row = sqlx::query(
            "INSERT INTO identity.users (id, email) \
             VALUES ($1, $2) \
             ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email \
             RETURNING id, status::text",
        )
        .bind(Uuid::now_v7())
        .bind(email)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let user_id: Uuid = row.try_get("id").map_err(database_error)?;
        let status: String = row.try_get("status").map_err(database_error)?;
        if status != "active" {
            transaction.commit().await.map_err(database_error)?;
            return Err(ApplicationError::Forbidden);
        }

        let grant = insert_session(&mut transaction, user_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(grant)
    }

    pub async fn authenticate_session(
        &self,
        token: &SecretString,
    ) -> Result<UserId, ApplicationError> {
        let digest = decode_and_digest_token(token)?;
        let user_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT session.user_id \
             FROM identity.sessions AS session \
             JOIN identity.users AS users ON users.id = session.user_id \
             WHERE session.token_digest = $1 \
               AND session.revoked_at IS NULL \
               AND session.expires_at > CURRENT_TIMESTAMP \
               AND users.status = 'active'",
        )
        .bind(digest.as_slice())
        .fetch_optional(&self.postgres)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        Ok(UserId::from_uuid(user_id))
    }

    pub async fn revoke_session(&self, token: &SecretString) -> Result<(), ApplicationError> {
        let digest = decode_and_digest_token(token)?;
        let result = sqlx::query(
            "UPDATE identity.sessions \
             SET revoked_at = CURRENT_TIMESTAMP \
             WHERE token_digest = $1 \
               AND revoked_at IS NULL \
               AND expires_at > CURRENT_TIMESTAMP",
        )
        .bind(digest.as_slice())
        .execute(&self.postgres)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(ApplicationError::Unauthorized);
        }
        Ok(())
    }

    async fn start_passkey_registration(
        &self,
        user_id: UserId,
    ) -> Result<CeremonyOptions<CreationChallengeResponse>, ApplicationError> {
        let row = sqlx::query("SELECT email::text FROM identity.users WHERE id = $1")
            .bind(user_id.as_uuid())
            .fetch_optional(&self.postgres)
            .await
            .map_err(database_error)?
            .ok_or(ApplicationError::Unauthorized)?;
        let email: String = row.try_get(0).map_err(database_error)?;
        let passkeys = self.load_passkeys(user_id.as_uuid()).await?;
        let exclude_credentials = (!passkeys.is_empty())
            .then(|| passkeys.iter().map(|item| item.cred_id().clone()).collect());
        let (public_key, state) = self
            .webauthn
            .start_passkey_registration(user_id.as_uuid(), &email, &email, exclude_credentials)
            .map_err(unexpected_webauthn_error)?;
        let ceremony_id = Uuid::now_v7();
        self.store_ceremony(
            "registration",
            ceremony_id,
            &RegistrationCeremony {
                user_id: user_id.as_uuid(),
                state,
            },
        )
        .await?;
        Ok(CeremonyOptions {
            ceremony_id,
            public_key,
        })
    }

    pub async fn finish_passkey_registration(
        &self,
        user_id: UserId,
        ceremony_id: Uuid,
        credential: RegisterPublicKeyCredential,
        name: &str,
    ) -> Result<Uuid, ApplicationError> {
        let ceremony: RegistrationCeremony =
            self.consume_ceremony("registration", ceremony_id).await?;
        if ceremony.user_id != user_id.as_uuid() {
            return Err(ApplicationError::Unauthorized);
        }
        let passkey = self
            .webauthn
            .finish_passkey_registration(&credential, &ceremony.state)
            .map_err(invalid_webauthn_credential)?;
        let id = Uuid::now_v7();
        let credential = serde_json::to_value(&passkey)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        sqlx::query(
            "INSERT INTO identity.passkey_credentials \
             (id, user_id, credential_id, credential, name) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(user_id.as_uuid())
        .bind(passkey.cred_id().as_ref())
        .bind(credential)
        .bind(name)
        .execute(&self.postgres)
        .await
        .map_err(map_passkey_insert_error)?;
        Ok(id)
    }

    async fn start_passkey_authentication(
        &self,
        email: Email,
    ) -> Result<CeremonyOptions<RequestChallengeResponse>, ApplicationError> {
        if !self
            .consume_rate_limit(
                "passkey-authentication",
                email.as_str(),
                PASSKEY_AUTHENTICATION_LIMIT,
                PASSKEY_AUTHENTICATION_WINDOW_SECONDS,
            )
            .await?
        {
            return Err(ApplicationError::Unauthorized);
        }
        let user_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM identity.users WHERE email = $1 AND status = 'active'",
        )
        .bind(email.as_str())
        .fetch_optional(&self.postgres)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        let passkeys = self.load_passkeys(user_id).await?;
        if passkeys.is_empty() {
            return Err(ApplicationError::Unauthorized);
        }
        let (public_key, state) = self
            .webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(unexpected_webauthn_error)?;
        let ceremony_id = Uuid::now_v7();
        self.store_ceremony(
            "authentication",
            ceremony_id,
            &AuthenticationCeremony { user_id, state },
        )
        .await?;
        Ok(CeremonyOptions {
            ceremony_id,
            public_key,
        })
    }

    pub async fn finish_passkey_authentication(
        &self,
        ceremony_id: Uuid,
        credential: PublicKeyCredential,
    ) -> Result<SessionGrant, ApplicationError> {
        let ceremony: AuthenticationCeremony =
            self.consume_ceremony("authentication", ceremony_id).await?;
        let result = self
            .webauthn
            .finish_passkey_authentication(&credential, &ceremony.state)
            .map_err(|_| ApplicationError::Unauthorized)?;
        let row = sqlx::query(
            "SELECT id, credential FROM identity.passkey_credentials \
             WHERE user_id = $1 AND credential_id = $2",
        )
        .bind(ceremony.user_id)
        .bind(result.cred_id().as_ref())
        .fetch_optional(&self.postgres)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::Unauthorized)?;
        let passkey_id: Uuid = row.try_get("id").map_err(database_error)?;
        let mut passkey: Passkey =
            serde_json::from_value(row.try_get("credential").map_err(database_error)?)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        passkey
            .update_credential(&result)
            .ok_or(ApplicationError::Unauthorized)?;
        let credential = serde_json::to_value(passkey)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;

        let mut transaction = self.postgres.begin().await.map_err(database_error)?;
        sqlx::query(
            "UPDATE identity.passkey_credentials \
             SET credential = $1, updated_at = CURRENT_TIMESTAMP, \
                 last_used_at = CURRENT_TIMESTAMP \
             WHERE id = $2",
        )
        .bind(credential)
        .bind(passkey_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let grant = insert_session(&mut transaction, ceremony.user_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(grant)
    }

    async fn load_passkeys(&self, user_id: Uuid) -> Result<Vec<Passkey>, ApplicationError> {
        let values = sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT credential FROM identity.passkey_credentials \
             WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id)
        .fetch_all(&self.postgres)
        .await
        .map_err(database_error)?;
        values
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))
            })
            .collect()
    }

    async fn store_ceremony<T: Serialize>(
        &self,
        kind: &str,
        id: Uuid,
        state: &T,
    ) -> Result<(), ApplicationError> {
        let key = ceremony_key(kind, id);
        let value = serde_json::to_string(state)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_error)?;
        connection
            .set_ex::<_, _, ()>(key, value, WEBAUTHN_CEREMONY_LIFETIME_SECONDS)
            .await
            .map_err(redis_error)
    }

    async fn consume_ceremony<T: for<'de> Deserialize<'de>>(
        &self,
        kind: &str,
        id: Uuid,
    ) -> Result<T, ApplicationError> {
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_error)?;
        let value: Option<String> = redis::cmd("GETDEL")
            .arg(ceremony_key(kind, id))
            .query_async(&mut connection)
            .await
            .map_err(redis_error)?;
        let value = value.ok_or(ApplicationError::Unauthorized)?;
        serde_json::from_str(&value).map_err(|error| ApplicationError::Unexpected(error.into()))
    }

    async fn consume_rate_limit(
        &self,
        scope: &str,
        subject: &str,
        limit: i64,
        window_seconds: i64,
    ) -> Result<bool, ApplicationError> {
        let subject_digest = URL_SAFE_NO_PAD.encode(token_digest(subject));
        let key = format!("chaos:auth:rate:{scope}:{subject_digest}");
        let script = redis::Script::new(
            "local count = redis.call('INCR', KEYS[1])\n\
             if count == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end\n\
             return count <= tonumber(ARGV[2])",
        );
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_error)?;
        script
            .key(key)
            .arg(window_seconds)
            .arg(limit)
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)
    }
}

async fn insert_session(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<SessionGrant, ApplicationError> {
    let token = random_token();
    let digest = token_digest(token.expose_secret());
    sqlx::query(
        "INSERT INTO identity.sessions (id, user_id, token_digest, expires_at) \
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP + make_interval(secs => $4))",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(digest.as_slice())
    .bind(SESSION_LIFETIME_SECONDS)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(SessionGrant {
        token,
        expires_in_seconds: SESSION_LIFETIME_SECONDS as u32,
    })
}

fn random_token() -> SecretString {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    SecretString::from(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_and_digest_token(token: &SecretString) -> Result<[u8; 32], ApplicationError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(token.expose_secret())
        .map_err(|_| ApplicationError::Unauthorized)?;
    if decoded.len() != 32 {
        return Err(ApplicationError::Unauthorized);
    }
    Ok(token_digest(token.expose_secret()))
}

fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn ceremony_key(kind: &str, id: Uuid) -> String {
    format!("chaos:auth:webauthn:{kind}:{id}")
}

fn map_passkey_insert_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("passkey_credentials_credential_id_key")
    {
        return ApplicationError::Conflict {
            code: "passkey_already_registered",
            message: "this passkey is already registered",
        };
    }
    database_error(error)
}

fn invalid_webauthn_credential(_error: webauthn_rs::prelude::WebauthnError) -> ApplicationError {
    invalid_webauthn_credential_placeholder()
}

fn invalid_webauthn_credential_placeholder() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "credential",
            reason: "must be a valid response for the active WebAuthn ceremony".into(),
        }],
    }
}

fn unexpected_webauthn_error(error: webauthn_rs::prelude::WebauthnError) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(error))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn redis_error(error: redis::RedisError) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "redis",
        source: error.into(),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[test]
    fn generated_tokens_have_256_bits_of_random_material() {
        let token = random_token();
        let decoded = URL_SAFE_NO_PAD.decode(token.expose_secret()).unwrap();

        assert_eq!(decoded.len(), 32);
        assert!(decode_and_digest_token(&token).is_ok());
    }

    #[test]
    fn malformed_tokens_are_rejected_before_database_access() {
        let token = SecretString::from("not-a-valid-session-token".to_owned());

        assert!(matches!(
            decode_and_digest_token(&token),
            Err(ApplicationError::Unauthorized)
        ));
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn control_plane_role_cannot_access_merchant_tables() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();

        sqlx::query("SET ROLE chaos_control_plane")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM identity.users LIMIT 1")
            .execute(&mut *connection)
            .await
            .unwrap();
        let merchant_access = sqlx::query("SELECT id FROM merchant.merchant_accounts LIMIT 1")
            .execute(&mut *connection)
            .await;

        assert!(merchant_access.is_err());
    }
}
