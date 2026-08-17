use async_trait::async_trait;
use chaos_domain::{
    merchant::StoreId,
    pricing::{Promotion, PromotionId, PromotionStatus},
};
use time::OffsetDateTime;

use super::{AdminActor, IdempotencyRequest};
use crate::ApplicationError;

pub struct PromotionDetail {
    pub promotion: Promotion,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait PromotionRepository: Send + Sync {
    async fn create_promotion(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        promotion: &Promotion,
        request: &IdempotencyRequest,
    ) -> Result<PromotionDetail, ApplicationError>;
    async fn list_promotions(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<PromotionDetail>, ApplicationError>;
    async fn change_promotion_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        promotion_id: PromotionId,
        status: PromotionStatus,
        request: &IdempotencyRequest,
    ) -> Result<PromotionDetail, ApplicationError>;
}
