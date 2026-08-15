use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    fulfillment::{
        FulfillmentId, FulfillmentStatus, ReturnDisposition, ReturnId, ReturnStatus,
        ShippingLabelId, ShippingProviderAccount, ShippingProviderAccountId,
        ShippingQuoteRequestId, ShippingRateQuoteId, ShippingSecretReference, ShippingService,
        ShippingServiceId, ShippingServiceStatus,
    },
    inventory::InventoryLocationId,
    merchant::StoreId,
    payments::RefundId,
    sales::OrderId,
};
use secrecy::SecretString;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

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
    pub refund_id: Option<RefundId>,
    pub refund_amount_minor: i64,
    pub currency: CurrencyCode,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct FulfillmentEventJob {
    pub id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

#[derive(Clone)]
pub struct ShippingServiceDetail {
    pub service: ShippingService,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct ShippingProviderAccountConfiguration {
    pub credential_secret_reference: ShippingSecretReference,
    pub origin: ShippingAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingProviderAccountDetail {
    pub account: ShippingProviderAccount,
    pub credentials_configured: bool,
    pub origin: ShippingAddress,
    pub credential_rotation_expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait ShippingProviderAccountRepository: Send + Sync {
    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError>;

    async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: ShippingProviderAccountId,
    ) -> Result<Option<ShippingProviderAccountDetail>, ApplicationError>;

    async fn create(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError>;

    async fn update(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingRateQuoteDetail {
    pub id: ShippingRateQuoteId,
    pub quote_request_id: ShippingQuoteRequestId,
    pub rate: ShippingRateQuote,
    pub expires_at: OffsetDateTime,
}

pub enum PreparedShippingQuote {
    Completed(Vec<ShippingRateQuoteDetail>),
    Pending {
        quote_request_id: ShippingQuoteRequestId,
        provider: String,
        command: Box<ShippingQuoteCommand>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingLabelDetail {
    pub id: ShippingLabelId,
    pub fulfillment_id: FulfillmentId,
    pub rate_quote_id: ShippingRateQuoteId,
    pub provider: String,
    pub label: PurchasedShippingLabel,
    pub cancellation_status: Option<ShippingCancellationStatus>,
    pub tracking: Option<ShippingTrackingSnapshot>,
    pub purchased_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub enum PreparedShippingLabelPurchase {
    Completed(Box<ShippingLabelDetail>),
    Pending {
        label_id: ShippingLabelId,
        provider: String,
        provider_shipment_reference: String,
        provider_rate_reference: String,
        credential_secret_reference: ShippingSecretReference,
        operation_key: String,
    },
}

pub enum PreparedShippingLabelCancellation {
    Completed(Box<ShippingLabelDetail>),
    Pending {
        label_id: ShippingLabelId,
        provider: String,
        provider_shipment_reference: String,
        credential_secret_reference: ShippingSecretReference,
    },
}

pub struct ShippingTrackingJob {
    pub label_id: ShippingLabelId,
    pub merchant_account_id: Uuid,
    pub store_id: StoreId,
    pub fulfillment_id: FulfillmentId,
    pub provider: String,
    pub provider_tracker_reference: String,
    pub credential_secret_reference: ShippingSecretReference,
    pub attempts: u32,
}

pub struct ShippingCancellationJob {
    pub label_id: ShippingLabelId,
    pub merchant_account_id: Uuid,
    pub store_id: StoreId,
    pub fulfillment_id: FulfillmentId,
    pub provider: String,
    pub provider_shipment_reference: String,
    pub credential_secret_reference: ShippingSecretReference,
    pub attempts: u32,
}

#[async_trait]
pub trait ShippingOperationRepository: Send + Sync {
    async fn prepare_quote(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        provider_account_id: ShippingProviderAccountId,
        parcel: ShippingParcel,
        idempotency: &IdempotencyRequest,
    ) -> Result<PreparedShippingQuote, ApplicationError>;

    async fn complete_quote(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        quote_request_id: ShippingQuoteRequestId,
        rates: Vec<ShippingRateQuote>,
        completed_at: OffsetDateTime,
    ) -> Result<Vec<ShippingRateQuoteDetail>, ApplicationError>;

    async fn prepare_label_purchase(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        rate_quote_id: ShippingRateQuoteId,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<PreparedShippingLabelPurchase, ApplicationError>;

    async fn complete_label_purchase(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        label_id: ShippingLabelId,
        label: PurchasedShippingLabel,
        completed_at: OffsetDateTime,
    ) -> Result<ShippingLabelDetail, ApplicationError>;

    async fn prepare_label_cancellation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<PreparedShippingLabelCancellation, ApplicationError>;

    async fn complete_label_cancellation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        label_id: ShippingLabelId,
        status: ShippingCancellationStatus,
        completed_at: OffsetDateTime,
    ) -> Result<ShippingLabelDetail, ApplicationError>;
}

#[async_trait]
pub trait ShippingTrackingQueue: Send + Sync {
    async fn claim_tracking(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<ShippingTrackingJob>, ApplicationError>;

    async fn finish_tracking(
        &self,
        worker_id: Uuid,
        job: &ShippingTrackingJob,
        result: Result<ShippingTrackingSnapshot, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn claim_cancellations(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<ShippingCancellationJob>, ApplicationError>;

    async fn finish_cancellation(
        &self,
        worker_id: Uuid,
        job: &ShippingCancellationJob,
        result: Result<ShippingCancellationStatus, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
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
    async fn get_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
    ) -> Result<Option<ReturnDetail>, ApplicationError>;

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

#[async_trait]
pub trait FulfillmentEventQueue: Send + Sync {
    async fn claim_events(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<FulfillmentEventJob>, ApplicationError>;

    async fn process_event(
        &self,
        job: &FulfillmentEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_event(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingAddress {
    pub name: String,
    pub company: Option<String>,
    pub address_line_1: String,
    pub address_line_2: Option<String>,
    pub city: String,
    pub region: Option<String>,
    pub postal_code: String,
    pub country_code: String,
    pub phone: Option<String>,
    pub email: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShippingParcel {
    pub length_millimetres: u32,
    pub width_millimetres: u32,
    pub height_millimetres: u32,
    pub weight_grams: u32,
}

pub struct ShippingQuoteCommand {
    pub from: ShippingAddress,
    pub to: ShippingAddress,
    pub parcel: ShippingParcel,
    pub idempotency_key: String,
    pub credential_secret_reference: ShippingSecretReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingRateQuote {
    pub provider_shipment_reference: String,
    pub provider_rate_reference: String,
    pub carrier: String,
    pub service: String,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub estimated_delivery_days: Option<u16>,
    pub guaranteed: bool,
}

pub struct PurchaseShippingLabelCommand {
    pub provider_shipment_reference: String,
    pub provider_rate_reference: String,
    pub idempotency_key: String,
    pub credential_secret_reference: ShippingSecretReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchasedShippingLabel {
    pub provider_shipment_reference: String,
    pub carrier: String,
    pub tracking_number: String,
    pub provider_tracker_reference: Option<String>,
    pub label_url: String,
    pub label_media_type: String,
}

pub struct CancelShippingLabelCommand {
    pub provider_shipment_reference: String,
    pub credential_secret_reference: ShippingSecretReference,
}

pub struct ReconcileShippingLabelCommand {
    pub provider_shipment_reference: String,
    pub credential_secret_reference: ShippingSecretReference,
}

pub struct ReconciledShippingLabel {
    pub label: Option<PurchasedShippingLabel>,
    pub cancellation_status: Option<ShippingCancellationStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShippingCancellationStatus {
    Submitted,
    Cancelled,
    Rejected,
    NotAvailable,
}

pub struct RefreshTrackingCommand {
    pub provider_tracker_reference: String,
    pub credential_secret_reference: ShippingSecretReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderTrackingStatus {
    PreTransit,
    InTransit,
    OutForDelivery,
    Delivered,
    Failure,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingTrackingSnapshot {
    pub status: ProviderTrackingStatus,
    pub status_detail: Option<String>,
    pub estimated_delivery_at: Option<OffsetDateTime>,
    pub observed_at: OffsetDateTime,
}

#[async_trait]
pub trait ShippingSecretResolver: Send + Sync {
    async fn resolve(
        &self,
        reference: &ShippingSecretReference,
    ) -> Result<SecretString, ApplicationError>;
}

#[async_trait]
pub trait ShippingProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn quote_rates(
        &self,
        command: ShippingQuoteCommand,
    ) -> Result<Vec<ShippingRateQuote>, ApplicationError>;

    async fn purchase_label(
        &self,
        command: PurchaseShippingLabelCommand,
    ) -> Result<PurchasedShippingLabel, ApplicationError>;

    async fn reconcile_label(
        &self,
        command: ReconcileShippingLabelCommand,
    ) -> Result<ReconciledShippingLabel, ApplicationError>;

    async fn cancel_label(
        &self,
        command: CancelShippingLabelCommand,
    ) -> Result<ShippingCancellationStatus, ApplicationError>;

    async fn refresh_tracking(
        &self,
        command: RefreshTrackingCommand,
        observed_at: OffsetDateTime,
    ) -> Result<ShippingTrackingSnapshot, ApplicationError>;
}
