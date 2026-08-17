use std::sync::Arc;

use chaos_domain::{
    merchant::{MerchantRole, StoreId},
    sales::{OrderId, OrderStatus},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        AdminActor, IdempotencyRequest, OrderDetail, OrderListFilter, OrderManagementRepository,
        OrderPage,
    },
};

pub struct ChangeOrderStatusInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub target_status: OrderStatus,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct OrderManagement {
    repository: Arc<dyn OrderManagementRepository>,
}

impl OrderManagement {
    pub fn new(repository: Arc<dyn OrderManagementRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_order(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
    ) -> Result<OrderDetail, ApplicationError> {
        self.repository
            .get_order(actor, store_id, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))
    }

    pub async fn list_orders(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<uuid::Uuid>,
        limit: u16,
        filter: OrderListFilter,
    ) -> Result<OrderPage, ApplicationError> {
        self.repository
            .list_orders(actor, store_id, after, limit, &filter)
            .await
    }

    pub async fn change_status(
        &self,
        input: ChangeOrderStatusInput,
    ) -> Result<OrderDetail, ApplicationError> {
        require_operator(input.actor)?;
        if !matches!(
            input.target_status,
            OrderStatus::Confirmed | OrderStatus::Cancelled
        ) {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "status",
                    reason: "must be confirmed or cancelled".into(),
                }],
            });
        }
        self.repository
            .transition_order(
                input.actor,
                input.store_id,
                input.order_id,
                input.target_status,
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

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}
