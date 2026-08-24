// Storefront sales repository core imports, row shapes, wiring, and shared constants.

use std::collections::HashMap;

use crate::{
    ApplicationError,
    error::database_error,
    contracts::{
        CartDetail, CartLineItem, MachineActor, OrderDetail,
        ShopperActor, StorefrontMediaAsset,
        StripeCheckoutDraft,
    },
    sales::StripeCheckoutRequest,
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    pricing::{Money, PriceListId},
    sales::{
        Cart, CartId, CartLine, CartStatus, OrderId, OrderNumber, ShopperId,
    },
    store::SalesChannelId,
};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::adapters::postgres::{
    analytics::{AnalyticsEventToAppend, append_event},
};

const ORDER_NUMBER_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn generate_order_number(now: OffsetDateTime) -> Result<OrderNumber, ApplicationError> {
    let mut random = [0_u8; 8];
    rand::rng().fill_bytes(&mut random);
    let suffix: String = random
        .into_iter()
        .map(|byte| char::from(ORDER_NUMBER_ALPHABET[usize::from(byte & 31)]))
        .collect();
    let date = now.date();
    OrderNumber::parse(format!(
        "W-{:04}{:02}{:02}-{suffix}",
        date.year(),
        u8::from(date.month()),
        date.day()
    ))
    .map_err(ApplicationError::from)
}
type CartHeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);

type CartLineRow = (Uuid, Uuid, String, String, Option<String>, bool, i32, i64);
type CartMediaRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    String,
    String,
    i16,
    String,
);
#[derive(Clone)]
pub struct PostgresStorefrontSalesRepository {
    pool: PgPool,
}

impl PostgresStorefrontSalesRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_store_context(&mut transaction, actor.store_id)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }

    async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin(&shopper.machine).await?;
        crate::adapters::postgres::database::set_shopper_context(&mut transaction, shopper.shopper_id)
            .await
            .map_err(database_error)?;
        sqlx::query(
            "UPDATE commerce.shoppers \
             SET last_seen_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(shopper.machine.store_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

// Shared ownership checks and workflow parsing.

async fn ensure_cart_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.carts \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
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

async fn ensure_order_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.orders \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(order_not_found(order_id))
    }
}

fn require_channel(actor: &MachineActor) -> Result<SalesChannelId, ApplicationError> {
    actor.sales_channel_id.ok_or(ApplicationError::Forbidden)
}

fn parse_currency(value: &str) -> Result<CurrencyCode, ApplicationError> {
    CurrencyCode::parse(value).map_err(ApplicationError::from)
}

fn payment_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no configured Payment Provider account is available",
    }
}

fn payment_provider_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_mismatch",
        message: "the idempotent checkout request selected another Payment Provider",
    }
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn cart_not_found(cart_id: CartId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: cart_id.as_uuid().to_string(),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn cart_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_not_active",
        message: "the Cart is no longer active",
    }
}

fn price_context_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "price_context_unavailable",
        message: "no active Price List is available for the requested currency",
    }
}

fn variant_unavailable(variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product_variant",
        id: variant_id.as_uuid().to_string(),
    }
}

fn cart_line_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_line_unavailable",
        message: "one or more Cart lines are no longer published and priced",
    }
}

fn insufficient_inventory(_variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::Conflict {
        code: "insufficient_inventory",
        message: "one or more Cart lines exceed available inventory",
    }
}

fn corrupt_sales_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown sales state"))
}
