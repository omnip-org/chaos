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
pub enum OrderPaymentStatus {
    Pending,
    Paid,
    Failed,
    Expired,
    PartiallyRefunded,
    Refunded,
}

impl OrderPaymentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::PartiallyRefunded => "partially_refunded",
            Self::Refunded => "refunded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "paid" => Some(Self::Paid),
            "failed" => Some(Self::Failed),
            "expired" => Some(Self::Expired),
            "partially_refunded" => Some(Self::PartiallyRefunded),
            "refunded" => Some(Self::Refunded),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderShippingStatus {
    Pending,
    AwaitingPickup,
    Shipped,
    Delivered,
    Cancelled,
}

impl OrderShippingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::AwaitingPickup => "awaiting_pickup",
            Self::Shipped => "shipped",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "awaiting_pickup" => Some(Self::AwaitingPickup),
            "shipped" => Some(Self::Shipped),
            "delivered" => Some(Self::Delivered),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
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

    pub fn confirm(&mut self) -> Result<(), DomainError> {
        self.transition(OrderStatus::Confirmed)
    }

    pub fn cancel(&mut self) -> Result<(), DomainError> {
        self.transition(OrderStatus::Cancelled)
    }

    fn transition(&mut self, to_status: OrderStatus) -> Result<(), DomainError> {
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
        self.status = to_status;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_allows_one_terminal_transition() {
        let mut order = Order::create();

        order.confirm().unwrap();

        assert_eq!(order.status(), OrderStatus::Confirmed);
        assert!(order.cancel().is_err());
    }

    #[test]
    fn cancelled_order_cannot_be_confirmed() {
        let mut order = Order::create();

        order.cancel().unwrap();

        assert_eq!(order.status(), OrderStatus::Cancelled);
        assert!(order.confirm().is_err());
    }

    #[test]
    fn order_number_is_bounded_readable_and_non_sequential() {
        let number = OrderNumber::parse("W-20260820-7K4M9Q2D").unwrap();
        assert_eq!(number.as_str(), "W-20260820-7K4M9Q2D");
        assert!(OrderNumber::parse("W-20260820-000001").is_err());
        assert!(OrderNumber::parse("W-20260820-ILOU1234").is_err());
    }
}
