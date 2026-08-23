use async_trait::async_trait;
use chaos_domain::{catalog::ProductVariantId, store::StoreId};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::AdminActor;

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

#[async_trait]
pub trait InventoryRepository: Send + Sync {
    async fn adjust_variant_inventory(
        &self,
        actor: AdminActor,
        adjustment: &InventoryAdjustment,
    ) -> Result<VariantInventoryView, ApplicationError>;

    async fn list_variant_inventory(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductVariantId>,
        limit: u16,
    ) -> Result<Option<Vec<VariantInventoryView>>, ApplicationError>;
}
