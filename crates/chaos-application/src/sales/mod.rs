use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    sales::{CartId, OrderContact, OrderId},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        CartDetail, IdempotencyRequest, MachineActor, ShopperActor, StorefrontSalesRepository,
        StripeCheckoutDraft,
    },
};

mod order_management;
pub use order_management::{ChangeOrderStatusInput, OrderManagement};

pub struct CreateCartInput {
    pub actor: ShopperActor,
    pub currency: Option<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct SetCartLineInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
    pub idempotency: IdempotencyRequest,
}

pub struct RemoveCartLineInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateStripeCheckoutInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub email: String,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct StorefrontSales {
    repository: Arc<dyn StorefrontSalesRepository>,
}

impl StorefrontSales {
    pub fn new(repository: Arc<dyn StorefrontSalesRepository>) -> Self {
        Self { repository }
    }

    pub async fn create_shopper(
        &self,
        actor: &MachineActor,
    ) -> Result<chaos_domain::sales::ShopperId, ApplicationError> {
        require_storefront_actor(actor)?;
        self.repository.create_shopper(actor).await
    }

    pub async fn create_cart(
        &self,
        input: CreateCartInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_actor(&input.actor.machine)?;
        let currency = input
            .currency
            .as_deref()
            .map(CurrencyCode::parse)
            .transpose()?;
        self.repository
            .create_cart(&input.actor, currency, &input.idempotency)
            .await
    }

    pub async fn get_cart(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_actor(&actor.machine)?;
        self.repository
            .get_cart(actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))
    }

    pub async fn set_cart_line(
        &self,
        input: SetCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_actor(&input.actor.machine)?;
        if !(1..=999).contains(&input.quantity) {
            return Err(validation("quantity", "must be between 1 and 999"));
        }
        self.repository
            .set_cart_line(
                &input.actor,
                input.cart_id,
                input.product_variant_id,
                input.quantity,
                &input.idempotency,
            )
            .await
    }

    pub async fn remove_cart_line(
        &self,
        input: RemoveCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_actor(&input.actor.machine)?;
        self.repository
            .remove_cart_line(
                &input.actor,
                input.cart_id,
                input.product_variant_id,
                &input.idempotency,
            )
            .await
    }

    pub async fn create_stripe_checkout(
        &self,
        input: CreateStripeCheckoutInput,
    ) -> Result<StripeCheckoutDraft, ApplicationError> {
        require_storefront_actor(&input.actor.machine)?;
        let contact = OrderContact::new(input.email, None)?;
        self.repository
            .create_stripe_checkout(
                &input.actor,
                input.cart_id,
                contact.email(),
                input.now,
                input.now + Duration::minutes(30),
                &input.idempotency,
            )
            .await
    }

    pub async fn get_order(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
    ) -> Result<crate::ports::OrderDetail, ApplicationError> {
        require_storefront_actor(&actor.machine)?;
        self.repository
            .get_order(actor, order_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "order",
                id: order_id.as_uuid().to_string(),
            })
    }

    pub async fn get_tracked_order(
        &self,
        actor: &MachineActor,
        tracking_token: &secrecy::SecretString,
        now: OffsetDateTime,
    ) -> Result<crate::ports::OrderDetail, ApplicationError> {
        require_storefront_actor(actor)?;
        self.repository
            .get_tracked_order(actor, tracking_token, now)
            .await?
            .ok_or(ApplicationError::Forbidden)
    }
}

fn require_storefront_actor(actor: &MachineActor) -> Result<(), ApplicationError> {
    if actor.sales_channel_id.is_some() {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn cart_not_found(cart_id: CartId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: cart_id.as_uuid().to_string(),
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
