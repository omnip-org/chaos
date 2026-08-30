use std::sync::Arc;

use chaos_domain::{
    catalog::{
        ProductId, ReviewContent, ReviewId, ReviewOrigin, ReviewRating, ReviewStatus,
        StaffReplyContent,
    },
    identity::Email,
    store::StoreId,
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresReviewRepository,
    contracts::{AdminActor, MachineActor, ReviewSummary},
    store::Page,
};

pub struct SubmitReviewInput {
    pub actor: MachineActor,
    pub product_id: ProductId,
    pub rating: u8,
    pub title: Option<String>,
    pub content: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub now: OffsetDateTime,
}

pub struct CreateManualReviewInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub rating: u8,
    pub title: Option<String>,
    pub content: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub source_channel: String,
    pub source_reference: Option<String>,
    pub now: OffsetDateTime,
}

pub struct ApproveReviewInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub verified_buyer: bool,
    pub now: OffsetDateTime,
}

pub struct RejectReviewInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub now: OffsetDateTime,
}

pub struct AddReviewReplyInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub parent_review_id: ReviewId,
    pub content: String,
    pub now: OffsetDateTime,
}

pub struct ReviewAdministration {
    repository: Arc<PostgresReviewRepository>,
}

impl ReviewAdministration {
    pub fn new(repository: Arc<PostgresReviewRepository>) -> Self {
        Self { repository }
    }

    /// Public Storefront submission: no shopper credential, no moderation role —
    /// any active Publishable Key may submit on a customer's behalf. The review
    /// always starts `pending` and is invisible until an
    /// administrator approves it.
    pub async fn submit(&self, input: SubmitReviewInput) -> Result<ReviewId, ApplicationError> {
        input.actor.require_sales_channel()?;
        let rating = ReviewRating::parse(input.rating)?;
        let author_email = input.author_email.map(Email::parse).transpose()?;
        let content = ReviewContent::new(
            rating,
            input.title,
            input.content,
            input.author_name,
            author_email,
        )?;
        self.repository
            .submit(
                &input.actor,
                crate::contracts::SubmitReviewRecord {
                    id: ReviewId::new(),
                    store_id: input.actor.store_id,
                    product_id: input.product_id,
                    content,
                    origin: ReviewOrigin::Storefront,
                    source_channel: None,
                    source_reference: None,
                    created_by_user_id: None,
                    created_at: input.now,
                },
            )
            .await
    }

    pub async fn create_manual(
        &self,
        input: CreateManualReviewInput,
    ) -> Result<ReviewId, ApplicationError> {
        input.actor.require_human()?;
        let source_channel = input.source_channel.trim().to_owned();
        validate_bounded_text(&source_channel, "source_channel", 80)?;
        let source_reference = input.source_reference.map(|value| value.trim().to_owned());
        if let Some(source_reference) = &source_reference {
            validate_bounded_text(source_reference, "source_reference", 255)?;
        }
        let rating = ReviewRating::parse(input.rating)?;
        let author_email = input.author_email.map(Email::parse).transpose()?;
        let content = ReviewContent::new(
            rating,
            input.title,
            input.content,
            input.author_name,
            author_email,
        )?;
        self.repository
            .create_manual(
                input.actor.clone(),
                crate::contracts::CreateManualReviewRecord {
                    id: ReviewId::new(),
                    store_id: input.store_id,
                    product_id: input.product_id,
                    content,
                    source_channel,
                    source_reference,
                    created_by_user_id: input.actor.audit_user_id(),
                    created_at: input.now,
                },
            )
            .await
    }

    pub async fn list_by_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: ReviewStatus,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Page<ReviewSummary>, ApplicationError> {
        actor.require_human()?;
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_by_status(actor, store_id, status, after, limit + 1)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "store",
                id: store_id.as_uuid().to_string(),
            })?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn approve(&self, input: ApproveReviewInput) -> Result<ReviewId, ApplicationError> {
        input.actor.require_human()?;
        self.repository
            .set_status(
                input.actor,
                input.store_id,
                input.review_id,
                ReviewStatus::Approved,
                input.verified_buyer,
                input.now,
            )
            .await
    }

    pub async fn reject(&self, input: RejectReviewInput) -> Result<ReviewId, ApplicationError> {
        input.actor.require_human()?;
        self.repository
            .set_status(
                input.actor,
                input.store_id,
                input.review_id,
                ReviewStatus::Rejected,
                false,
                input.now,
            )
            .await
    }

    pub async fn add_reply(
        &self,
        input: AddReviewReplyInput,
    ) -> Result<ReviewId, ApplicationError> {
        input.actor.require_human()?;
        let content = StaffReplyContent::new(input.content)?;
        self.repository
            .add_reply(
                input.actor,
                input.store_id,
                input.parent_review_id,
                content,
                input.now,
            )
            .await
    }
}

fn validate_bounded_text(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), ApplicationError> {
    let empty = value.trim().is_empty();
    if empty || value.chars().count() > maximum || value.chars().any(char::is_control) {
        return Err(validation(
            field,
            "must contain non-control characters within the allowed length",
        ));
    }
    Ok(())
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

pub struct StorefrontReviews {
    repository: Arc<PostgresReviewRepository>,
}

impl StorefrontReviews {
    pub fn new(repository: Arc<PostgresReviewRepository>) -> Self {
        Self { repository }
    }

    /// Approved reviews (and their approved replies) for one Product.
    /// The repository packs each top-level review together with its own replies
    /// into one flat, ordered list — `limit` bounds top-level reviews, not the
    /// combined row count, so pagination is trimmed by counting top-level items
    /// rather than the raw list length.
    pub async fn list_for_product(
        &self,
        actor: &MachineActor,
        product_id: ProductId,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Page<ReviewSummary>, ApplicationError> {
        actor.require_sales_channel()?;
        let limit = limit.clamp(1, 100);
        let items = self
            .repository
            .list_approved_for_product(actor, product_id, after, limit + 1)
            .await?;
        let top_level = usize::from(limit);
        let mut seen = 0usize;
        let mut cutoff = items.len();
        for (index, item) in items.iter().enumerate() {
            if item.parent_review_id.is_none() {
                seen += 1;
                if seen > top_level {
                    cutoff = index;
                    break;
                }
            }
        }
        let has_more = seen > top_level;
        let mut items = items;
        items.truncate(cutoff);
        Ok(Page { items, has_more })
    }
}
