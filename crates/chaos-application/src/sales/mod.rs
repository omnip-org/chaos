use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    merchant::{ApiKeyClass, ApiKeyScope},
    sales::{CartId, CheckoutId, OrderId},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        CartDetail, CheckoutDetail, IdempotencyRequest, MachineActor, StorefrontSalesRepository,
    },
};

mod order_management;
pub use order_management::{ChangeOrderStatusInput, OrderManagement};

pub struct CreateCartInput {
    pub actor: MachineActor,
    pub currency: Option<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct SetCartLineInput {
    pub actor: MachineActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
    pub idempotency: IdempotencyRequest,
}

pub struct RemoveCartLineInput {
    pub actor: MachineActor,
    pub cart_id: CartId,
    pub product_variant_id: ProductVariantId,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateCheckoutInput {
    pub actor: MachineActor,
    pub cart_id: CartId,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateOrderInput {
    pub actor: MachineActor,
    pub checkout_id: CheckoutId,
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

    pub async fn create_cart(
        &self,
        input: CreateCartInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_scope(&input.actor, ApiKeyScope::CartsWrite)?;
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
        actor: &MachineActor,
        cart_id: CartId,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_scope(actor, ApiKeyScope::CartsWrite)?;
        self.repository
            .get_cart(actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))
    }

    pub async fn set_cart_line(
        &self,
        input: SetCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_scope(&input.actor, ApiKeyScope::CartsWrite)?;
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
        require_storefront_scope(&input.actor, ApiKeyScope::CartsWrite)?;
        self.repository
            .remove_cart_line(
                &input.actor,
                input.cart_id,
                input.product_variant_id,
                &input.idempotency,
            )
            .await
    }

    pub async fn create_checkout(
        &self,
        input: CreateCheckoutInput,
    ) -> Result<CheckoutDetail, ApplicationError> {
        require_storefront_scope(&input.actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .create_checkout(
                &input.actor,
                input.cart_id,
                input.now,
                input.now + Duration::minutes(15),
                &input.idempotency,
            )
            .await
    }

    pub async fn get_checkout(
        &self,
        actor: &MachineActor,
        checkout_id: CheckoutId,
    ) -> Result<CheckoutDetail, ApplicationError> {
        require_storefront_scope(actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .get_checkout(actor, checkout_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "checkout",
                id: checkout_id.as_uuid().to_string(),
            })
    }

    pub async fn create_order(
        &self,
        input: CreateOrderInput,
    ) -> Result<crate::ports::OrderDetail, ApplicationError> {
        require_storefront_scope(&input.actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .create_order(
                &input.actor,
                input.checkout_id,
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn get_order(
        &self,
        actor: &MachineActor,
        order_id: OrderId,
    ) -> Result<crate::ports::OrderDetail, ApplicationError> {
        require_storefront_scope(actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .get_order(actor, order_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "order",
                id: order_id.as_uuid().to_string(),
            })
    }
}

fn require_storefront_scope(
    actor: &MachineActor,
    required_scope: ApiKeyScope,
) -> Result<(), ApplicationError> {
    if actor.class == ApiKeyClass::Publishable
        && actor.sales_channel_id.is_some()
        && actor.scopes.contains(&required_scope)
    {
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
