use std::sync::Arc;

use chaos_domain::{
    catalog::{ProductId, ReviewContent, ReviewId, ReviewRating, ReviewStatus, StaffReplyContent},
    identity::Email,
    merchant::{ApiKeyScope, StoreId},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::Page,
    ports::{AdminActor, IdempotencyRequest, MachineActor, ReviewRepository, ReviewSummary},
};

pub struct SubmitReviewInput {
    pub actor: MachineActor,
    pub product_id: ProductId,
    pub rating: u8,
    pub title: Option<String>,
    pub content: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ApproveReviewInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub verified_buyer: bool,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct RejectReviewInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct AddReviewReplyInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub parent_review_id: ReviewId,
    pub content: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ReviewAdministration {
    repository: Arc<dyn ReviewRepository>,
}

impl ReviewAdministration {
    pub fn new(repository: Arc<dyn ReviewRepository>) -> Self {
        Self { repository }
    }

    /// Public Storefront submission: no shopper credential, no moderation role —
    /// any Publishable key holding `reviews:write` may submit on a customer's
    /// behalf. The review always starts `pending` and is invisible until an
    /// administrator approves it.
    pub async fn submit(&self, input: SubmitReviewInput) -> Result<ReviewId, ApplicationError> {
        require_storefront_scope(&input.actor, ApiKeyScope::ReviewsWrite)?;
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
                crate::ports::SubmitReviewRecord {
                    id: ReviewId::new(),
                    store_id: input.actor.store_id,
                    product_id: input.product_id,
                    content,
                    created_at: input.now,
                },
                &input.idempotency,
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
        require_moderator(&actor)?;
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
        require_moderator(&input.actor)?;
        self.repository
            .set_status(
                input.actor,
                input.store_id,
                input.review_id,
                ReviewStatus::Approved,
                input.verified_buyer,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn reject(&self, input: RejectReviewInput) -> Result<ReviewId, ApplicationError> {
        require_moderator(&input.actor)?;
        self.repository
            .set_status(
                input.actor,
                input.store_id,
                input.review_id,
                ReviewStatus::Rejected,
                false,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn add_reply(
        &self,
        input: AddReviewReplyInput,
    ) -> Result<ReviewId, ApplicationError> {
        require_moderator(&input.actor)?;
        let content = StaffReplyContent::new(input.content)?;
        self.repository
            .add_reply(
                input.actor,
                input.store_id,
                input.parent_review_id,
                content,
                &input.idempotency,
                input.now,
            )
            .await
    }
}

pub struct StorefrontReviews {
    repository: Arc<dyn ReviewRepository>,
}

impl StorefrontReviews {
    pub fn new(repository: Arc<dyn ReviewRepository>) -> Self {
        Self { repository }
    }

    /// Approved reviews (and their approved replies) for one Product. Requires
    /// only `catalog:read` — the same scope every other public catalog read uses.
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
        require_storefront_scope(actor, ApiKeyScope::CatalogRead)?;
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

fn require_moderator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(machine) => {
            if machine.scopes.contains(&ApiKeyScope::ReviewsWrite) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}

fn require_storefront_scope(
    actor: &MachineActor,
    required_scope: ApiKeyScope,
) -> Result<(), ApplicationError> {
    if actor.class == chaos_domain::merchant::ApiKeyClass::Publishable
        && actor.sales_channel_id.is_some()
        && actor.scopes.contains(&required_scope)
    {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}
