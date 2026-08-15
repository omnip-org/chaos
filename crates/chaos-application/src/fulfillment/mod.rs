use std::sync::Arc;

use chaos_domain::{
    CurrencyCode,
    fulfillment::{FulfillmentId, FulfillmentStatus, ReturnId, ReturnStatus},
    merchant::{MerchantRole, StoreId},
    pricing::Money,
    sales::OrderId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, FulfillmentEventQueue,
        FulfillmentRepository, IdempotencyRequest, ReturnDetail, ReturnLineInput,
        ReturnReceiptInput, ShippingServiceDetail, ShippingServiceRepository,
    },
};
use chaos_domain::fulfillment::{ShippingService, ShippingServiceId, ShippingServiceStatus};

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

pub struct FulfillmentWorkers {
    queue: Arc<dyn FulfillmentEventQueue>,
}

impl FulfillmentWorkers {
    pub fn new(queue: Arc<dyn FulfillmentEventQueue>) -> Self {
        Self { queue }
    }

    pub async fn run_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_events(worker_id, limit, now, now - Duration::minutes(1))
            .await?;
        for job in &jobs {
            let result = self
                .queue
                .process_event(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_event(worker_id, job.id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }
}

pub struct CreateShippingServiceInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub currency: String,
    pub amount_minor: i64,
    pub estimated_min_days: u16,
    pub estimated_max_days: u16,
    pub destination_countries: Vec<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeShippingServiceStatusInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub service_id: ShippingServiceId,
    pub status: ShippingServiceStatus,
    pub idempotency: IdempotencyRequest,
}

pub struct ShippingManagement {
    repository: Arc<dyn ShippingServiceRepository>,
}

impl ShippingManagement {
    pub fn new(repository: Arc<dyn ShippingServiceRepository>) -> Self {
        Self { repository }
    }

    pub async fn create(
        &self,
        input: CreateShippingServiceInput,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        require_operator(input.actor)?;
        let currency = CurrencyCode::parse(&input.currency)?;
        let service = ShippingService::create(
            input.code,
            input.name,
            Money::new(input.amount_minor, currency),
            input.estimated_min_days,
            input.estimated_max_days,
            input.destination_countries,
        )?;
        self.repository
            .create_shipping_service(input.actor, input.store_id, &service, &input.idempotency)
            .await
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingServiceDetail>, ApplicationError> {
        require_operator(actor)?;
        self.repository
            .list_shipping_services(actor, store_id)
            .await
    }

    pub async fn change_status(
        &self,
        input: ChangeShippingServiceStatusInput,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        require_operator(input.actor)?;
        self.repository
            .change_shipping_service_status(
                input.actor,
                input.store_id,
                input.service_id,
                input.status,
                &input.idempotency,
            )
            .await
    }
}

impl FulfillmentManagement {
    pub fn new(repository: Arc<dyn FulfillmentRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
    ) -> Result<ReturnDetail, ApplicationError> {
        require_operator(actor)?;
        self.repository
            .get_return(actor, store_id, return_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "return",
                id: return_id.as_uuid().to_string(),
            })
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
