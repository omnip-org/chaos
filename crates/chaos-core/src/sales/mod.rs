use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    catalog::ProductVariantId,
    integration::PaymentProvider,
    sales::{CartId, OrderContact},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresStorefrontSalesRepository,
    contracts::{CartDetail, CheckoutDraft, MachineActor, ShopperActor},
};

mod order_management;
pub use order_management::{ChangeOrderStatusInput, OrderManagement};

pub struct CreateCartInput {
    pub actor: ShopperActor,
}

pub struct SetCartLineInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
    pub expected_version: u64,
}

pub struct RemoveCartLineInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub expected_version: u64,
}

pub struct CreateStripeCheckoutInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    /// Optional: Stripe Embedded Checkout collects the shopper's email
    /// directly when the storefront does not already have one, and a
    /// verified payment webhook backfills it onto the Order afterward.
    pub email: Option<String>,
    pub return_url: String,
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: uuid::Uuid,
}

pub(crate) struct StripeCheckoutRequest {
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: uuid::Uuid,
    pub return_url: String,
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
    ) -> Result<chaos_domain::sales::ShopperId, ApplicationError> {
        actor.require_sales_channel()?;
        self.repository.create_shopper(actor).await
    }

    pub async fn create_cart(
        &self,
        input: CreateCartInput,
    ) -> Result<CartDetail, ApplicationError> {
        input.actor.machine.require_sales_channel()?;
        self.repository.create_cart(&input.actor).await
    }

    pub async fn get_cart(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
    ) -> Result<CartDetail, ApplicationError> {
        actor.machine.require_sales_channel()?;
        self.repository
            .get_cart(actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))
    }

    pub async fn set_cart_line(
        &self,
        input: SetCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        input.actor.machine.require_sales_channel()?;
        if !(1..=999).contains(&input.quantity) {
            return Err(validation("quantity", "must be between 1 and 999"));
        }
        self.repository
            .set_cart_line(
                &input.actor,
                input.cart_id,
                input.product_variant_id,
                input.quantity,
                input.expected_version,
            )
            .await
    }

    pub async fn remove_cart_line(
        &self,
        input: RemoveCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        input.actor.machine.require_sales_channel()?;
        self.repository
            .remove_cart_line(
                &input.actor,
                input.cart_id,
                input.product_variant_id,
                input.expected_version,
            )
            .await
    }

    pub async fn create_stripe_checkout(
        &self,
        input: CreateStripeCheckoutInput,
    ) -> Result<CheckoutDraft, ApplicationError> {
        input.actor.machine.require_sales_channel()?;
        let contact = OrderContact::new(input.email, None)?;
        self.repository
            .create_stripe_checkout(
                &input.actor,
                input.cart_id,
                contact.email(),
                StripeCheckoutRequest {
                    payment_provider: input.payment_provider,
                    now: input.now,
                    idempotency_key: input.idempotency_key,
                    return_url: input.return_url,
                },
            )
            .await
    }

    /// Guest Order lookup by the printed Order number and the contact email on
    /// the Order. Both must match within the caller's Store and Sales Channel;
    /// every miss — unknown number, wrong email, malformed input — returns the
    /// same `NotFound` so the endpoint is not an Order-number oracle.
    pub async fn lookup_order(
        &self,
        actor: &MachineActor,
        order_number: &str,
        email: &str,
    ) -> Result<crate::contracts::OrderDetail, ApplicationError> {
        actor.require_sales_channel()?;
        self.repository
            .lookup_order(actor, order_number, email)
            .await?
            .ok_or(ApplicationError::NotFound {
                resource: "order",
                id: order_number.to_owned(),
            })
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
