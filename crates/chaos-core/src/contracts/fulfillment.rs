use chaos_domain::{
    fulfillment::{FulfillmentId, FulfillmentStatus, ShippingProviderAccountId},
    integration::ShippingProvider,
    sales::OrderId,
};
use time::OffsetDateTime;

pub struct ShippingProviderAccountDetail {
    pub id: ShippingProviderAccountId,
    pub provider: ShippingProvider,
    pub display_name: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct FulfillmentDetail {
    pub id: FulfillmentId,
    pub order_id: OrderId,
    pub shipping_provider_account_id: ShippingProviderAccountId,
    pub provider_reference_id: Option<String>,
    pub status: FulfillmentStatus,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
    pub shipped_at: Option<OffsetDateTime>,
    pub delivered_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}
