use std::sync::Arc;

use chaos_domain::{
    fulfillment::{FulfillmentId, FulfillmentStatus, ReturnId, ReturnStatus},
    merchant::{MerchantRole, StoreId},
    sales::OrderId,
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, FulfillmentRepository, IdempotencyRequest,
        ReturnDetail, ReturnLineInput, ReturnReceiptInput,
    },
};

pub struct CreateFulfillmentInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub allocations: Vec<FulfillmentAllocationInput>,
    pub idempotency: IdempotencyRequest,
}

pub struct TransitionFulfillmentInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub fulfillment_id: FulfillmentId,
    pub target_status: FulfillmentStatus,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateReturnInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub lines: Vec<ReturnLineInput>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct TransitionReturnInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub return_id: ReturnId,
    pub target_status: ReturnStatus,
    pub receipt: Vec<ReturnReceiptInput>,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct FulfillmentManagement {
    repository: Arc<dyn FulfillmentRepository>,
}

impl FulfillmentManagement {
    pub fn new(repository: Arc<dyn FulfillmentRepository>) -> Self {
        Self { repository }
    }

    pub async fn create_fulfillment(
        &self,
        input: CreateFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .create_fulfillment(
                input.actor,
                input.store_id,
                input.order_id,
                input.allocations,
                &input.idempotency,
            )
            .await
    }

    pub async fn transition_fulfillment(
        &self,
        input: TransitionFulfillmentInput,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .transition_fulfillment(
                input.actor,
                input.store_id,
                input.fulfillment_id,
                input.target_status,
                input.carrier.as_deref(),
                input.tracking_number.as_deref(),
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn create_return(
        &self,
        input: CreateReturnInput,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .create_return(
                input.actor,
                input.store_id,
                input.order_id,
                input.lines,
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn transition_return(
        &self,
        input: TransitionReturnInput,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .transition_return(
                input.actor,
                input.store_id,
                input.return_id,
                input.target_status,
                input.receipt,
                input.now,
                &input.idempotency,
            )
            .await
    }
}

fn require_operator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator | MerchantRole::Manager
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}
