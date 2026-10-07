use crate::{
    ApplicationError,
    contracts::{MachineActor, OrderDetail, ShopperActor},
    error::database_error,
    sales::CheckoutRequest,
};
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    sales::{CartId, OrderId, OrderNumber, ShopperId},
    store::SalesChannelId,
};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const ORDER_NUMBER_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// `W-` plus eight Crockford base32 characters, giving roughly 40 random bits.
pub(super) fn generate_order_number() -> Result<OrderNumber, ApplicationError> {
    let mut random = [0_u8; 8];
    rand::rng().fill_bytes(&mut random);
    let suffix: String = random
        .into_iter()
        .map(|byte| char::from(ORDER_NUMBER_ALPHABET[usize::from(byte & 31)]))
        .collect();
    OrderNumber::parse(format!("W-{suffix}")).map_err(|error| {
        ApplicationError::Unexpected(anyhow::anyhow!(
            "generated order number failed validation: {error}"
        ))
    })
}

pub(super) fn order_number_unavailable() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("could not allocate a unique order number"))
}

#[derive(Clone)]
pub struct PostgresStorefrontSalesRepository {
    pool: PgPool,
}

impl PostgresStorefrontSalesRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(super) async fn begin(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_store_context(&mut transaction, actor.store_id)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }

    pub(super) async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin(&shopper.machine).await?;
        crate::adapters::postgres::database::set_shopper_context(
            &mut transaction,
            shopper.shopper_id,
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

pub(super) async fn ensure_cart_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM chaos_commerce.carts \
         WHERE store_id = $1 AND channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(cart_not_found(cart_id))
    }
}

pub(super) fn require_channel(actor: &MachineActor) -> Result<SalesChannelId, ApplicationError> {
    actor.channel_id.ok_or(ApplicationError::Forbidden)
}

pub(super) fn parse_currency(value: &str) -> Result<CurrencyCode, ApplicationError> {
    CurrencyCode::parse(value).map_err(ApplicationError::from)
}

pub(super) fn payment_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no configured Payment Provider account is available",
    }
}

pub(super) fn idempotency_key_reused() -> ApplicationError {
    ApplicationError::Conflict {
        code: "idempotency_key_reused",
        message: "the idempotency key was already used with different checkout parameters",
    }
}

fn fingerprint_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

pub(super) fn checkout_request_fingerprint(
    actor: &MachineActor,
    request: &CheckoutRequest,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"chaos-checkout-request-v5");
    fingerprint_part(&mut hasher, actor.store_id.as_uuid().as_bytes());
    fingerprint_part(
        &mut hasher,
        actor
            .channel_id
            .map(SalesChannelId::as_uuid)
            .unwrap_or(Uuid::nil())
            .as_bytes(),
    );
    fingerprint_part(&mut hasher, request.payment_provider.as_str().as_bytes());
    hasher.finalize().into()
}

pub(super) fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

pub(super) fn checkout_insert_error(error: sqlx::Error) -> ApplicationError {
    let constraint = match &error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    };
    match constraint {
        Some("carts_checkout_idempotency_key_key") => idempotency_key_reused(),
        Some("orders_one_order_per_cart_key") => checkout_cart_already_started(),
        _ => database_error(error),
    }
}

pub(super) fn cart_not_found(cart_id: CartId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: cart_id.as_uuid().to_string(),
    }
}

pub(super) fn cart_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_not_active",
        message: "the Cart is no longer active",
    }
}

pub(super) fn checkout_cart_already_started() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_cart_already_started",
        message: "the Cart is locked to an existing checkout; retry the same Cart checkout request or use a new active Cart",
    }
}

pub(super) fn price_context_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "price_context_unavailable",
        message: "no active Price List is available for the requested currency",
    }
}

pub(super) fn variant_unavailable(variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product_variant",
        id: variant_id.as_uuid().to_string(),
    }
}

pub(super) fn cart_line_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_line_unavailable",
        message: "one or more Cart lines are no longer published and priced",
    }
}

pub(super) fn insufficient_inventory(_variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::Conflict {
        code: "insufficient_inventory",
        message: "one or more Cart lines exceed available inventory",
    }
}

pub(super) fn corrupt_sales_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown sales state"))
}

pub(super) async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    crate::adapters::postgres::sales::order_detail::load(
        transaction,
        actor.store_id,
        actor.channel_id,
        order_id,
    )
    .await
}
