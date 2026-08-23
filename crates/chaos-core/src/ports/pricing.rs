use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    pricing::{PriceListId, PriceListStatus},
};
use time::OffsetDateTime;

pub struct PriceListReadItem {
    pub id: PriceListId,
    pub code: String,
    pub name: String,
    pub currency: CurrencyCode,
    pub status: PriceListStatus,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub price_count: u32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct PriceReadItem {
    pub product_variant_id: ProductVariantId,
    pub amount_minor: i64,
}

pub struct PriceListDetail {
    pub item: PriceListReadItem,
    pub prices: Vec<PriceReadItem>,
}

pub struct PriceListMutationSnapshot {
    pub status: PriceListStatus,
    pub priced_variant_ids: Vec<ProductVariantId>,
}
