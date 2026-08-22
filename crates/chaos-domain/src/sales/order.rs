use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, FieldViolation};

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OrderNumber(String);

impl OrderNumber {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = bytes.len() == 19
            && &bytes[..2] == b"W-"
            && bytes[2..10].iter().all(u8::is_ascii_digit)
            && bytes[10] == b'-'
            && bytes[11..]
                .iter()
                .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(byte));
        if !valid {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "order_number",
                reason: "must use the W-YYYYMMDD-XXXXXXXX format".into(),
            }]));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
pub enum OrderFulfillmentStatus {
    Unfulfilled,
    PartiallyFulfilled,
    Fulfilled,
}

impl OrderFulfillmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unfulfilled => "unfulfilled",
            Self::PartiallyFulfilled => "partially_fulfilled",
            Self::Fulfilled => "fulfilled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unfulfilled" => Some(Self::Unfulfilled),
            "partially_fulfilled" => Some(Self::PartiallyFulfilled),
            "fulfilled" => Some(Self::Fulfilled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderDeliveryStatus {
    NotDelivered,
    PartiallyDelivered,
    Delivered,
}

impl OrderDeliveryStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotDelivered => "not_delivered",
            Self::PartiallyDelivered => "partially_delivered",
            Self::Delivered => "delivered",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "not_delivered" => Some(Self::NotDelivered),
            "partially_delivered" => Some(Self::PartiallyDelivered),
            "delivered" => Some(Self::Delivered),
            _ => None,
        }
    }
}

pub fn reconcile_fulfillment_statuses(
    total_shippable_quantity: u64,
    shipped_quantity: u64,
    delivered_quantity: u64,
) -> Result<(OrderFulfillmentStatus, OrderDeliveryStatus), DomainError> {
    if shipped_quantity > total_shippable_quantity || delivered_quantity > shipped_quantity {
        return Err(DomainError::Validation(vec![FieldViolation {
            field: "fulfillment_quantity",
            reason: "derived quantities must satisfy delivered <= shipped <= shippable".into(),
        }]));
    }
    let fulfillment = if shipped_quantity == 0 {
        OrderFulfillmentStatus::Unfulfilled
    } else if shipped_quantity == total_shippable_quantity {
        OrderFulfillmentStatus::Fulfilled
    } else {
        OrderFulfillmentStatus::PartiallyFulfilled
    };
    let delivery = if delivered_quantity == 0 {
        OrderDeliveryStatus::NotDelivered
    } else if delivered_quantity == total_shippable_quantity {
        OrderDeliveryStatus::Delivered
    } else {
        OrderDeliveryStatus::PartiallyDelivered
    };
    Ok((fulfillment, delivery))
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
    status: OrderStatus,
}

impl Order {
    pub fn create() -> Self {
        Self {
            id: OrderId::new(),
            status: OrderStatus::Pending,
        }
    }

    pub fn rehydrate(id: OrderId, status: OrderStatus) -> Self {
        Self { id, status }
    }

    pub const fn id(&self) -> OrderId {
        self.id
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
        let now = OffsetDateTime::now_utc();
        let mut order = Order::create();

        let transition = order.confirm(now).unwrap();

        assert_eq!(transition.from_status, Some(OrderStatus::Pending));
        assert_eq!(transition.to_status, OrderStatus::Confirmed);
        assert_eq!(transition.kind, OrderTransitionKind::Confirmed);
        assert!(order.cancel(now).is_err());
    }

    #[test]
    fn cancelled_order_cannot_be_confirmed() {
        let now = OffsetDateTime::now_utc();
        let mut order = Order::create();

        order.cancel(now).unwrap();

        assert_eq!(order.status(), OrderStatus::Cancelled);
        assert!(order.confirm(now).is_err());
    }

    #[test]
    fn order_number_is_bounded_readable_and_non_sequential() {
        let number = OrderNumber::parse("W-20260820-7K4M9Q2D").unwrap();
        assert_eq!(number.as_str(), "W-20260820-7K4M9Q2D");
        assert!(OrderNumber::parse("W-20260820-000001").is_err());
        assert!(OrderNumber::parse("W-20260820-ILOU1234").is_err());
    }

    #[test]
    fn fulfillment_reconciliation_tracks_partial_and_complete_progress() {
        assert_eq!(
            reconcile_fulfillment_statuses(3, 2, 1).unwrap(),
            (
                OrderFulfillmentStatus::PartiallyFulfilled,
                OrderDeliveryStatus::PartiallyDelivered
            )
        );
        assert_eq!(
            reconcile_fulfillment_statuses(3, 3, 3).unwrap(),
            (
                OrderFulfillmentStatus::Fulfilled,
                OrderDeliveryStatus::Delivered
            )
        );
    }

    #[test]
    fn fulfillment_reconciliation_rejects_impossible_quantities() {
        assert!(reconcile_fulfillment_statuses(1, 2, 0).is_err());
        assert!(reconcile_fulfillment_statuses(2, 1, 2).is_err());
    }
}
