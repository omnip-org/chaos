use async_trait::async_trait;
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{
        FulfillmentId, FulfillmentStatus, ReturnDisposition, ReturnId, ReturnStatus,
        ShippingService, ShippingServiceId, ShippingServiceStatus,
    },
    inventory::InventoryLocationId,
    merchant::StoreId,
    sales::OrderId,
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

pub struct FulfillmentAllocationInput {
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
}

pub struct ReturnLineInput {
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
}

pub struct ReturnReceiptInput {
    pub product_variant_id: ProductVariantId,
    pub disposition: ReturnDisposition,
    pub inventory_location_id: Option<InventoryLocationId>,
}

pub struct FulfillmentDetail {
    pub id: FulfillmentId,
    pub order_id: OrderId,
    pub status: FulfillmentStatus,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub allocations: Vec<FulfillmentAllocationInput>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct ReturnDetail {
    pub id: ReturnId,
    pub order_id: OrderId,
    pub status: ReturnStatus,
    pub lines: Vec<ReturnLineInput>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct ShippingServiceDetail {
    pub service: ShippingService,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait ShippingServiceRepository: Send + Sync {
    async fn create_shipping_service(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        service: &ShippingService,
        idempotency: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError>;

    async fn list_shipping_services(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingServiceDetail>, ApplicationError>;

    async fn change_shipping_service_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        service_id: ShippingServiceId,
        status: ShippingServiceStatus,
        idempotency: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError>;
}

#[async_trait]
pub trait FulfillmentRepository: Send + Sync {
    async fn create_fulfillment(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
        allocations: Vec<FulfillmentAllocationInput>,
        idempotency: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn transition_fulfillment(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        target_status: FulfillmentStatus,
        carrier: Option<&str>,
        tracking_number: Option<&str>,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError>;

    async fn create_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
        lines: Vec<ReturnLineInput>,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn transition_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
        target_status: ReturnStatus,
        receipt: Vec<ReturnReceiptInput>,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError>;
}
