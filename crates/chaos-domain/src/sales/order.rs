use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, FieldViolation, sales::CheckoutId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrderId(Uuid);

impl OrderId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for OrderId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderStatus {
    Pending,
    Confirmed,
    Cancelled,
}

impl OrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "confirmed" => Some(Self::Confirmed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderTransitionKind {
    Created,
    Confirmed,
    Cancelled,
}

impl OrderTransitionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Confirmed => "confirmed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderTransition {
    pub from_status: Option<OrderStatus>,
    pub to_status: OrderStatus,
    pub kind: OrderTransitionKind,
    pub occurred_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    id: OrderId,
    checkout_id: CheckoutId,
    status: OrderStatus,
}

impl Order {
    pub fn create(checkout_id: CheckoutId) -> Self {
        Self {
            id: OrderId::new(),
            checkout_id,
            status: OrderStatus::Pending,
        }
    }

    pub fn rehydrate(id: OrderId, checkout_id: CheckoutId, status: OrderStatus) -> Self {
        Self {
            id,
            checkout_id,
            status,
        }
    }

    pub const fn id(&self) -> OrderId {
        self.id
    }

    pub const fn checkout_id(&self) -> CheckoutId {
        self.checkout_id
    }

    pub const fn status(&self) -> OrderStatus {
        self.status
    }

    pub fn confirm(&mut self, occurred_at: OffsetDateTime) -> Result<OrderTransition, DomainError> {
        self.transition(
            OrderStatus::Confirmed,
            OrderTransitionKind::Confirmed,
            occurred_at,
        )
    }

    pub fn cancel(&mut self, occurred_at: OffsetDateTime) -> Result<OrderTransition, DomainError> {
        self.transition(
            OrderStatus::Cancelled,
            OrderTransitionKind::Cancelled,
            occurred_at,
        )
    }

    fn transition(
        &mut self,
        to_status: OrderStatus,
        kind: OrderTransitionKind,
        occurred_at: OffsetDateTime,
    ) -> Result<OrderTransition, DomainError> {
        if self.status != OrderStatus::Pending {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "order_status",
                reason: format!(
                    "cannot transition from {} to {}",
                    self.status.as_str(),
                    to_status.as_str()
                ),
            }]));
        }
        let from_status = self.status;
        self.status = to_status;
        Ok(OrderTransition {
            from_status: Some(from_status),
            to_status,
            kind,
            occurred_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_allows_one_terminal_transition_and_records_it() {
        let checkout_id = CheckoutId::new();
        let now = OffsetDateTime::now_utc();
        let mut order = Order::create(checkout_id);

        let transition = order.confirm(now).unwrap();

        assert_eq!(transition.from_status, Some(OrderStatus::Pending));
        assert_eq!(transition.to_status, OrderStatus::Confirmed);
        assert_eq!(transition.kind, OrderTransitionKind::Confirmed);
        assert!(order.cancel(now).is_err());
    }

    #[test]
    fn cancelled_order_cannot_be_confirmed() {
        let now = OffsetDateTime::now_utc();
        let mut order = Order::create(CheckoutId::new());

        order.cancel(now).unwrap();

        assert_eq!(order.status(), OrderStatus::Cancelled);
        assert!(order.confirm(now).is_err());
    }
}
