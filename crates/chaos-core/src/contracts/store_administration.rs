use chaos_domain::{
    CurrencyCode, RegionCode,
    store::{SalesChannelId, SalesChannelStatus, StoreId, StoreStatus, StorefrontOrigin},
};
use time::OffsetDateTime;

pub struct StoreAdminItem {
    pub id: StoreId,
    pub name: String,
    pub region: RegionCode,
    pub currency: CurrencyCode,
    pub meta: Option<serde_json::Value>,
    pub status: StoreStatus,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct SalesChannelAdminItem {
    pub id: SalesChannelId,
    pub name: String,
    pub origin: StorefrontOrigin,
    pub status: SalesChannelStatus,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct ShippingCountryAdminItem {
    pub country_code: String,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}
