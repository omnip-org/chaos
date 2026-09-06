use chaos_domain::{
    fulfillment::{FulfillmentId, FulfillmentStatus, FulfillmentProviderAccountId},
    integration::FulfillmentProvider,
    sales::OrderId,
};
use time::OffsetDateTime;

pub struct FulfillmentProviderAccountDetail {
    pub id: FulfillmentProviderAccountId,
    pub provider: FulfillmentProvider,
    pub display_name: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct FulfillmentDetail {
    pub id: FulfillmentId,
    pub order_id: OrderId,
    pub provider_account_id: FulfillmentProviderAccountId,
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
