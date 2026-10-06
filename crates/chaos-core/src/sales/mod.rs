use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    catalog::ProductVariantId,
    integration::PaymentProvider,
    sales::{CartId, OrderId},
};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresStorefrontSalesRepository,
    contracts::{CartDetail, MachineActor, OrderDetail, ShopperActor},
};

mod attribution;
mod order_management;
pub use attribution::{CheckoutAttributionInput, ShopperSessionContext, UtmTags};
use attribution::{checkout_attribution_value, shopper_seen_snapshot, shopper_session_attribution};
pub use order_management::{ChangeOrderStatusInput, OrderManagement};

pub struct CreateCheckoutInput {
    pub shopper: ShopperActor,
    pub cart_id: CartId,
    pub return_url: String,
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: Uuid,
    pub attribution: CheckoutAttributionInput,
}

pub(crate) struct CheckoutRequest {
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: Uuid,
    pub return_url: String,
    pub attribution: Option<Value>,
}

pub struct StorefrontSales {
    repository: Arc<PostgresStorefrontSalesRepository>,
}

impl StorefrontSales {
    pub fn new(repository: Arc<PostgresStorefrontSalesRepository>) -> Self {
        Self { repository }
    }

    pub async fn create_shopper(
        &self,
        actor: &MachineActor,
        context: ShopperSessionContext,
    ) -> Result<chaos_domain::sales::ShopperId, ApplicationError> {
        actor.require_sales_channel()?;
        self.repository
            .create_shopper(actor, shopper_session_attribution(context))
            .await
    }

    pub async fn touch_shopper(
        &self,
        shopper: &ShopperActor,
        context: ShopperSessionContext,
    ) -> Result<(), ApplicationError> {
        shopper.machine.require_sales_channel()?;
        let Some(snapshot) = shopper_seen_snapshot(context) else {
            return Ok(());
        };
        self.repository
            .refresh_shopper_last_seen(shopper, snapshot)
            .await
    }

    pub async fn create_cart(&self, shopper: ShopperActor) -> Result<CartDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        self.repository.create_cart(&shopper).await
    }

    pub async fn get_active_cart(
        &self,
        shopper: &ShopperActor,
    ) -> Result<CartDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        self.repository
            .get_active_cart(shopper)
            .await?
            .ok_or_else(active_cart_not_found)
    }

    pub async fn get_cart(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
    ) -> Result<CartDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        self.repository.get_cart(shopper, cart_id).await
    }

    pub async fn set_cart_line(
        &self,
        shopper: ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
    ) -> Result<CartDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        if !(1..=999).contains(&quantity) {
            return Err(validation("quantity", "must be between 1 and 999"));
        }
        self.repository
            .set_cart_line(&shopper, cart_id, product_variant_id, quantity)
            .await
    }

    pub async fn remove_cart_line(
        &self,
        shopper: ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
    ) -> Result<CartDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        self.repository
            .remove_cart_line(&shopper, cart_id, product_variant_id)
            .await
    }

    pub async fn create_checkout(
        &self,
        input: CreateCheckoutInput,
    ) -> Result<OrderId, ApplicationError> {
        input.shopper.machine.require_sales_channel()?;
        self.repository
            .create_checkout(
                &input.shopper,
                input.cart_id,
                CheckoutRequest {
                    payment_provider: input.payment_provider,
                    now: input.now,
                    idempotency_key: input.idempotency_key,
                    return_url: input.return_url,
                    attribution: checkout_attribution_value(input.attribution),
                },
            )
            .await
    }

    pub async fn lookup_order(
        &self,
        actor: &MachineActor,
        order_number: &str,
        email: &str,
    ) -> Result<OrderDetail, ApplicationError> {
        actor.require_sales_channel()?;
        self.repository
            .lookup_order(actor, order_number, email)
            .await?
            .ok_or(ApplicationError::NotFound {
                resource: "order",
                id: order_number.to_owned(),
            })
    }

    pub async fn get_shopper_order(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<OrderDetail, ApplicationError> {
        shopper.machine.require_sales_channel()?;
        self.repository
            .get_shopper_order(shopper, order_id)
            .await?
            .ok_or(ApplicationError::NotFound {
                resource: "order",
                id: order_id.as_uuid().to_string(),
            })
    }
}

fn active_cart_not_found() -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: "active".into(),
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
