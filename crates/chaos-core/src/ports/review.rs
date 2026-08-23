use chaos_domain::{
    catalog::{ProductId, ReviewContent, ReviewId, ReviewStatus},
    store::StoreId,
};
use time::OffsetDateTime;

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
