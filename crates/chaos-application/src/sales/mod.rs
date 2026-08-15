use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    fulfillment::{ShippingSelection, ShippingServiceId},
    merchant::{ApiKeyClass, ApiKeyScope},
    sales::{CartId, CheckoutContact, CheckoutId, CheckoutIdentity, OrderId, PostalAddress},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        CartDetail, CheckoutDetail, CheckoutExpiryQueue, IdempotencyRequest, MachineActor,
        ShopperActor, StorefrontSalesRepository,
    },
};

mod order_management;
pub use order_management::{ChangeOrderStatusInput, OrderManagement};

const EXPIRY_LEASE_TIMEOUT: Duration = Duration::minutes(1);

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

pub struct CreateCheckoutInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub contact: CheckoutContactInput,
    pub billing_address: PostalAddressInput,
    pub shipping_address: Option<PostalAddressInput>,
    pub shipping_service_id: Option<ShippingServiceId>,
    pub promotion_code: Option<String>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct QuoteShippingInput {
    pub actor: ShopperActor,
    pub cart_id: CartId,
    pub destination_country: String,
}

pub struct CheckoutContactInput {
    pub email: String,
    pub phone: Option<String>,
}

pub struct PostalAddressInput {
    pub full_name: String,
    pub company: Option<String>,
    pub address_line1: String,
    pub address_line2: Option<String>,
    pub locality: String,
    pub administrative_area: Option<String>,
    pub postal_code: Option<String>,
    pub country_code: String,
}

pub struct CreateOrderInput {
    pub actor: ShopperActor,
    pub checkout_id: CheckoutId,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct StorefrontSales {
    repository: Arc<dyn StorefrontSalesRepository>,
}

pub struct CheckoutExpiryWorkers {
    queue: Arc<dyn CheckoutExpiryQueue>,
}

impl CheckoutExpiryWorkers {
    pub fn new(queue: Arc<dyn CheckoutExpiryQueue>) -> Self {
        Self { queue }
    }

    pub async fn run_batch(
        &self,
        worker_id: uuid::Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_due_checkouts(worker_id, limit, now, now - EXPIRY_LEASE_TIMEOUT)
            .await?;
        let mut first_error = None;
        for job in jobs.iter().copied() {
            if let Err(error) = self.queue.expire_checkout(worker_id, job, now).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(jobs.len())
    }
}

impl StorefrontSales {
    pub fn new(repository: Arc<dyn StorefrontSalesRepository>) -> Self {
        Self { repository }
    }

    pub async fn create_cart(
        &self,
        input: CreateCartInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
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
        require_storefront_scope(&actor.machine, ApiKeyScope::CartsWrite)?;
        self.repository
            .get_cart(actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))
    }

    pub async fn set_cart_line(
        &self,
        input: SetCartLineInput,
    ) -> Result<CartDetail, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
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
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CartsWrite)?;
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
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CheckoutWrite)?;
        let identity = CheckoutIdentity::new(
            CheckoutContact::new(input.contact.email, input.contact.phone)?,
            postal_address(input.billing_address)?,
            input.shipping_address.map(postal_address).transpose()?,
        );
        self.repository
            .create_checkout(
                &input.actor,
                input.cart_id,
                input.now,
                input.now + Duration::minutes(15),
                identity,
                input.shipping_service_id,
                input.promotion_code.as_deref(),
                &input.idempotency,
            )
            .await
    }

    pub async fn quote_shipping(
        &self,
        input: QuoteShippingInput,
    ) -> Result<Vec<ShippingSelection>, ApplicationError> {
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CheckoutWrite)?;
        let country = input.destination_country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(validation(
                "destination_country",
                "must be an ISO 3166-1 alpha-2 country code",
            ));
        }
        self.repository
            .quote_shipping(&input.actor, input.cart_id, &country)
            .await
    }

    pub async fn get_checkout(
        &self,
        actor: &ShopperActor,
        checkout_id: CheckoutId,
    ) -> Result<CheckoutDetail, ApplicationError> {
        require_storefront_scope(&actor.machine, ApiKeyScope::CheckoutWrite)?;
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
        require_storefront_scope(&input.actor.machine, ApiKeyScope::CheckoutWrite)?;
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
        actor: &ShopperActor,
        order_id: OrderId,
    ) -> Result<crate::ports::OrderDetail, ApplicationError> {
        require_storefront_scope(&actor.machine, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .get_order(actor, order_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "order",
                id: order_id.as_uuid().to_string(),
            })
    }
}

fn postal_address(input: PostalAddressInput) -> Result<PostalAddress, ApplicationError> {
    PostalAddress::new(
        input.full_name,
        input.company,
        input.address_line1,
        input.address_line2,
        input.locality,
        input.administrative_area,
        input.postal_code,
        input.country_code,
    )
    .map_err(ApplicationError::from)
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
