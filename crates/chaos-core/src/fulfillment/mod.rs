use std::sync::Arc;

use chaos_domain::{
    fulfillment::{FulfillmentId, ShippingProviderAccountId},
    sales::OrderId,
    store::{StoreId, StoreRole},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresFulfillmentRepository,
    contracts::{AdminActor, FulfillmentDetail, ShippingProviderAccountDetail},
};

pub struct CreateFulfillmentInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub shipping_provider_account_id: ShippingProviderAccountId,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
}

pub struct MarkShippedInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub id: FulfillmentId,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
    pub now: OffsetDateTime,
}

pub struct MarkDeliveredInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub id: FulfillmentId,
    pub now: OffsetDateTime,
}

pub struct CancelFulfillmentInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub id: FulfillmentId,
    pub now: OffsetDateTime,
}

pub struct FulfillmentManagement {
    repository: Arc<PostgresFulfillmentRepository>,
}

impl FulfillmentManagement {
    pub fn new(repository: Arc<PostgresFulfillmentRepository>) -> Self {
        Self { repository }
    }

    pub async fn list_shipping_provider_accounts(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError> {
        self.repository
            .list_shipping_provider_accounts(actor, store_id)
            .await
    }

    pub async fn create_fulfillment(
        &self,
        input: CreateFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_fulfillment_operator(&input.actor)?;
        self.repository
            .create_fulfillment(
                input.actor,
                input.store_id,
                input.order_id,
                input.shipping_provider_account_id,
                input.tracking_number,
                input.tracking_url,
            )
            .await
    }

    pub async fn mark_shipped(
        &self,
        input: MarkShippedInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_fulfillment_operator(&input.actor)?;
        self.repository
            .mark_shipped(
                input.actor,
                input.store_id,
                input.id,
                input.tracking_number,
                input.tracking_url,
                input.now,
            )
            .await
    }

    pub async fn mark_delivered(
        &self,
        input: MarkDeliveredInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_fulfillment_operator(&input.actor)?;
        self.repository
            .mark_delivered(input.actor, input.store_id, input.id, input.now)
            .await
    }

    pub async fn cancel(
        &self,
        input: CancelFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_fulfillment_operator(&input.actor)?;
        self.repository
            .cancel(input.actor, input.store_id, input.id, input.now)
            .await
    }
}

fn require_fulfillment_operator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(store_actor) => match store_actor.role() {
            StoreRole::Owner | StoreRole::Member => Ok(()),
        },
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}
