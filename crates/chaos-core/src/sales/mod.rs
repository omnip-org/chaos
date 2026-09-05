use std::sync::Arc;

use chaos_domain::{
    FieldViolation, catalog::ProductVariantId, integration::PaymentProvider, sales::CartId,
};
use serde_json::Value;
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
    pub return_url: String,
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: uuid::Uuid,
    /// Ad-platform attribution the browser read off its own cookies at
    /// checkout time (e.g. Meta's `fbc`/`fbp`). Stored on the Cart so the
    /// payment webhook can attach it to the server-side Purchase event later
    /// without correlating a separate browser-recorded event.
    pub attribution: Option<CheckoutAttributionInput>,
}

/// Namespaced by ad platform so a future platform is an additive field, not a
/// schema or contract change. Only Meta is wired up today: it's the only
/// platform Chaos sends a server-side conversion event for.
#[derive(Default)]
pub struct CheckoutAttributionInput {
    pub meta_fbc: Option<String>,
    pub meta_fbp: Option<String>,
    /// Captured by the API layer from the checkout request itself (not
    /// trusted from the client) — see `carts.rs`'s handler.
    pub client_ip_address: Option<String>,
    pub client_user_agent: Option<String>,
    /// The checkout page's own URL, read by the browser from
    /// `window.location`. Not platform-specific, so it sits alongside
    /// `meta_*` rather than inside a platform namespace.
    pub source_url: Option<String>,
}

pub(crate) struct StripeCheckoutRequest {
    pub payment_provider: PaymentProvider,
    pub now: OffsetDateTime,
    pub idempotency_key: uuid::Uuid,
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
        self.repository
            .create_stripe_checkout(
                &input.actor,
                input.cart_id,
                StripeCheckoutRequest {
                    payment_provider: input.payment_provider,
                    now: input.now,
                    idempotency_key: input.idempotency_key,
                    return_url: input.return_url,
                    attribution: checkout_attribution_value(input.attribution),
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

/// Drop invalid or oversized attribution rather than fail checkout over it —
/// this is enrichment for a later Meta CAPI call, not something a shopper's
/// purchase should ever block on. The Meta adapter re-checks `fbc`/`fbp`
/// format itself before sending, so this only needs to bound what's stored.
fn checkout_attribution_value(input: Option<CheckoutAttributionInput>) -> Option<Value> {
    let input = input?;
    let mut meta = serde_json::Map::new();
    for (key, value) in [
        ("fbc", input.meta_fbc),
        ("fbp", input.meta_fbp),
        ("client_ip_address", input.client_ip_address),
        ("client_user_agent", input.client_user_agent),
    ] {
        if let Some(value) = sanitized_attribution_string(value) {
            meta.insert(key.into(), Value::String(value));
        }
    }
    let mut attribution = serde_json::Map::new();
    if let Some(source_url) = sanitized_attribution_string(input.source_url) {
        attribution.insert("source_url".into(), Value::String(source_url));
    }
    if !meta.is_empty() {
        attribution.insert("meta".into(), Value::Object(meta));
    }
    (!attribution.is_empty()).then_some(Value::Object(attribution))
}

fn sanitized_attribution_string(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
    })
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
