use std::sync::Arc;

use chaos_domain::{
    CurrencyCode,
    merchant::StoreId,
    pricing::{Promotion, PromotionId, PromotionStatus, PromotionTrigger, PromotionValue},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{AdminActor, IdempotencyRequest, PromotionDetail, PromotionRepository},
};

pub struct CreatePromotionInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub handle: String,
    pub name: String,
    pub trigger: PromotionTrigger,
    pub redemption_code: Option<String>,
    pub value: PromotionValue,
    pub currency: CurrencyCode,
    pub minimum_subtotal_amount_minor: i64,
    pub priority: u16,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangePromotionStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub promotion_id: PromotionId,
    pub status: PromotionStatus,
    pub idempotency: IdempotencyRequest,
}

pub struct PromotionManagement {
    repository: Arc<dyn PromotionRepository>,
}

impl PromotionManagement {
    pub fn new(repository: Arc<dyn PromotionRepository>) -> Self {
        Self { repository }
    }
    pub async fn create(
        &self,
        input: CreatePromotionInput,
    ) -> Result<PromotionDetail, ApplicationError> {
        require_operator(&input.actor)?;
        let promotion = Promotion::create(
            input.handle,
            input.name,
            input.trigger,
            input.redemption_code,
            input.value,
            input.currency,
            input.minimum_subtotal_amount_minor,
            input.priority,
            input.starts_at,
            input.ends_at,
        )?;
        self.repository
            .create_promotion(input.actor, input.store_id, &promotion, &input.idempotency)
            .await
    }
    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<PromotionDetail>, ApplicationError> {
        require_operator(&actor)?;
        self.repository.list_promotions(actor, store_id).await
    }
    pub async fn change_status(
        &self,
        input: ChangePromotionStatusInput,
    ) -> Result<PromotionDetail, ApplicationError> {
        require_operator(&input.actor)?;
        self.repository
            .change_promotion_status(
                input.actor,
                input.store_id,
                input.promotion_id,
                input.status,
                &input.idempotency,
            )
            .await
    }
}

fn require_operator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}
