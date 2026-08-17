use async_trait::async_trait;
use chaos_domain::{
    catalog::{ProductId, ReviewContent, ReviewId, ReviewStatus, StaffReplyContent},
    merchant::StoreId,
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest, MachineActor};

pub struct ReviewSummary {
    pub id: ReviewId,
    pub product_id: ProductId,
    pub parent_review_id: Option<ReviewId>,
    pub rating: Option<u8>,
    pub title: Option<String>,
    pub content: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub status: ReviewStatus,
    pub is_staff_reply: bool,
    pub verified_buyer: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct SubmitReviewRecord {
    pub id: ReviewId,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub content: ReviewContent,
    pub created_at: OffsetDateTime,
}

#[async_trait]
pub trait ReviewRepository: Send + Sync {
    async fn submit(
        &self,
        actor: &MachineActor,
        record: SubmitReviewRecord,
        request: &IdempotencyRequest,
    ) -> Result<ReviewId, ApplicationError>;

    async fn list_by_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: ReviewStatus,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Option<Vec<ReviewSummary>>, ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn set_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        review_id: ReviewId,
        status: ReviewStatus,
        verified_buyer: bool,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError>;

    async fn add_reply(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        parent_review_id: ReviewId,
        content: StaffReplyContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError>;

    async fn list_approved_for_product(
        &self,
        actor: &MachineActor,
        product_id: ProductId,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Vec<ReviewSummary>, ApplicationError>;
}
