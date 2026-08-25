//! Database transaction contexts and database-specific infrastructure helpers.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(test)]
mod store_context;

use chaos_domain::{identity::UserId, sales::ShopperId, store::StoreId};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

pub(crate) const ORDER_TRACKING_TOKEN_LIFETIME: time::Duration = time::Duration::days(180);

pub(crate) struct OrderTrackingCapability {
    pub(crate) token: SecretString,
    pub(crate) digest: [u8; 32],
}

pub(crate) fn generate_order_tracking_capability() -> OrderTrackingCapability {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let token = SecretString::from(format!("ot_{}", URL_SAFE_NO_PAD.encode(secret)));
    let digest = Sha256::digest(token.expose_secret()).into();
    OrderTrackingCapability { token, digest }
}

pub(crate) async fn set_user_context(
    connection: &mut PgConnection,
    user_id: UserId,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(user_id.as_uuid().to_string())
        .execute(connection)
        .await
        .map(|_| ())
}

pub(crate) async fn clear_user_context(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.user_id', '', true)")
        .execute(connection)
        .await
        .map(|_| ())
}

pub(crate) async fn set_store_context(
    connection: &mut PgConnection,
    store_id: StoreId,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.store_id', $1, true)")
        .bind(store_id.as_uuid().to_string())
        .execute(connection)
        .await
        .map(|_| ())
}

pub(crate) async fn set_shopper_context(
    connection: &mut PgConnection,
    shopper_id: ShopperId,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.shopper_id', $1, true)")
        .bind(shopper_id.as_uuid().to_string())
        .execute(connection)
        .await
        .map(|_| ())
}

pub(crate) async fn set_admin_context(
    connection: &mut PgConnection,
    user_id: Option<UserId>,
    store_id: StoreId,
) -> Result<(), sqlx::Error> {
    set_optional_user_context(connection, user_id).await?;
    set_store_context(connection, store_id).await
}

pub(crate) async fn set_optional_user_context(
    connection: &mut PgConnection,
    user_id: Option<UserId>,
) -> Result<(), sqlx::Error> {
    match user_id {
        Some(user_id) => set_user_context(connection, user_id).await,
        None => clear_user_context(connection).await,
    }
}
