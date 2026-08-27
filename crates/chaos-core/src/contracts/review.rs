use chaos_domain::{
    catalog::{
        MediaAssetId, MediaAssetStatus, MediaKind, ProductId, ReviewContent, ReviewId,
        ReviewOrigin, ReviewStatus,
    },
    store::StoreId,
};
use time::OffsetDateTime;

#[derive(Clone)]
pub struct ReviewMediaSummary {
    pub id: MediaAssetId,
    pub media_type: String,
    pub kind: MediaKind,
    pub alt_text: String,
    pub position: u16,
    pub status: MediaAssetStatus,
    pub public_url: Option<String>,
}

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
    pub origin: ReviewOrigin,
    pub source_channel: Option<String>,
    pub source_reference: Option<String>,
    pub images: Vec<ReviewMediaSummary>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct SubmitReviewRecord {
    pub id: ReviewId,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub content: ReviewContent,
    pub origin: ReviewOrigin,
    pub source_channel: Option<String>,
    pub source_reference: Option<String>,
    pub publication_consent_confirmed: bool,
    pub created_by_user_id: Option<chaos_domain::identity::UserId>,
    pub created_at: OffsetDateTime,
}

pub struct CreateManualReviewRecord {
    pub id: ReviewId,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub content: ReviewContent,
    pub source_channel: String,
    pub source_reference: Option<String>,
    pub publication_consent_confirmed: bool,
    pub created_by_user_id: Option<chaos_domain::identity::UserId>,
    pub created_at: OffsetDateTime,
}
