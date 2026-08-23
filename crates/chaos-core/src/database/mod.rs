//! Database transaction contexts and database-specific infrastructure helpers.

pub mod store_context;

use chaos_domain::{identity::UserId, sales::ShopperId, store::StoreId};
use sqlx::PgConnection;

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
    user_id: UserId,
    store_id: StoreId,
) -> Result<(), sqlx::Error> {
    set_user_context(connection, user_id).await?;
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
