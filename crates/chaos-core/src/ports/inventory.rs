use chaos_domain::{catalog::ProductVariantId, store::StoreId};
use time::OffsetDateTime;

pub struct VariantInventoryView {
    pub product_variant_id: ProductVariantId,
    pub on_hand_quantity: i64,
    pub updated_at: OffsetDateTime,
}

pub struct InventoryAdjustment {
    pub store_id: StoreId,
    pub product_variant_id: ProductVariantId,
    pub delta_quantity: i64,
    pub note: String,
}
