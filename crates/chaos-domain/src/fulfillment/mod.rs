use uuid::Uuid;

use crate::{DomainError, FieldViolation, catalog::ProductVariantId, sales::OrderId};

macro_rules! operation_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
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

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

operation_id!(FulfillmentId);
operation_id!(ReturnId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FulfillmentStatus {
    Pending,
    Shipped,
    Delivered,
    Cancelled,
}

impl FulfillmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Shipped => "shipped",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "shipped" => Some(Self::Shipped),
            "delivered" => Some(Self::Delivered),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FulfillmentAllocation {
    pub product_variant_id: ProductVariantId,
    pub quantity: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fulfillment {
    id: FulfillmentId,
    order_id: OrderId,
    status: FulfillmentStatus,
    allocations: Vec<FulfillmentAllocation>,
}

impl Fulfillment {
    pub fn create(
        order_id: OrderId,
        mut allocations: Vec<FulfillmentAllocation>,
    ) -> Result<Self, DomainError> {
        if allocations.is_empty() {
            return Err(validation("lines", "must contain at least one allocation"));
        }
        allocations.sort_by_key(|line| line.product_variant_id.as_uuid());
        for (index, line) in allocations.iter().enumerate() {
            if line.quantity == 0 || line.quantity > 999 {
                return Err(validation("quantity", "must be between 1 and 999"));
            }
            if index > 0 && allocations[index - 1].product_variant_id == line.product_variant_id {
                return Err(validation("lines", "must not repeat a Variant"));
            }
        }
        Ok(Self {
            id: FulfillmentId::new(),
            order_id,
            status: FulfillmentStatus::Pending,
            allocations,
        })
    }

    pub fn rehydrate(
        id: FulfillmentId,
        order_id: OrderId,
        status: FulfillmentStatus,
        allocations: Vec<FulfillmentAllocation>,
    ) -> Self {
        Self {
            id,
            order_id,
            status,
            allocations,
        }
    }

    pub const fn id(&self) -> FulfillmentId {
        self.id
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn status(&self) -> FulfillmentStatus {
        self.status
    }

    pub fn allocations(&self) -> &[FulfillmentAllocation] {
        &self.allocations
    }

    pub fn ship(&mut self) -> Result<(), DomainError> {
        self.transition(FulfillmentStatus::Pending, FulfillmentStatus::Shipped)
    }

    pub fn deliver(&mut self) -> Result<(), DomainError> {
        self.transition(FulfillmentStatus::Shipped, FulfillmentStatus::Delivered)
    }

    pub fn cancel(&mut self) -> Result<(), DomainError> {
        self.transition(FulfillmentStatus::Pending, FulfillmentStatus::Cancelled)
    }

    fn transition(
        &mut self,
        expected: FulfillmentStatus,
        target: FulfillmentStatus,
    ) -> Result<(), DomainError> {
        if self.status != expected {
            return Err(validation(
                "status",
                "does not permit the requested Fulfillment transition",
            ));
        }
        self.status = target;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnStatus {
    Requested,
    Authorized,
    Received,
    Completed,
    Rejected,
}

impl ReturnStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Authorized => "authorized",
            Self::Received => "received",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "requested" => Some(Self::Requested),
            "authorized" => Some(Self::Authorized),
            "received" => Some(Self::Received),
            "completed" => Some(Self::Completed),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnDisposition {
    Restock,
    Discard,
}

impl ReturnDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Restock => "restock",
            Self::Discard => "discard",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "restock" => Some(Self::Restock),
            "discard" => Some(Self::Discard),
            _ => None,
        }
    }
}

pub struct Return {
    id: ReturnId,
    order_id: OrderId,
    status: ReturnStatus,
}

impl Return {
    pub fn create(order_id: OrderId) -> Self {
        Self {
            id: ReturnId::new(),
            order_id,
            status: ReturnStatus::Requested,
        }
    }

    pub fn rehydrate(id: ReturnId, order_id: OrderId, status: ReturnStatus) -> Self {
        Self {
            id,
            order_id,
            status,
        }
    }

    pub const fn id(&self) -> ReturnId {
        self.id
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn status(&self) -> ReturnStatus {
        self.status
    }

    pub fn authorize(&mut self) -> Result<(), DomainError> {
        self.transition(ReturnStatus::Requested, ReturnStatus::Authorized)
    }

    pub fn reject(&mut self) -> Result<(), DomainError> {
        self.transition(ReturnStatus::Requested, ReturnStatus::Rejected)
    }

    pub fn receive(&mut self) -> Result<(), DomainError> {
        self.transition(ReturnStatus::Authorized, ReturnStatus::Received)
    }

    pub fn complete(&mut self) -> Result<(), DomainError> {
        self.transition(ReturnStatus::Received, ReturnStatus::Completed)
    }

    fn transition(
        &mut self,
        expected: ReturnStatus,
        target: ReturnStatus,
    ) -> Result<(), DomainError> {
        if self.status != expected {
            return Err(validation(
                "status",
                "does not permit the requested Return transition",
            ));
        }
        self.status = target;
        Ok(())
    }
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fulfillment_supports_partial_allocations_and_shipping_boundaries() {
        let mut fulfillment = Fulfillment::create(
            OrderId::new(),
            vec![FulfillmentAllocation {
                product_variant_id: ProductVariantId::new(),
                quantity: 1,
            }],
        )
        .unwrap();

        fulfillment.ship().unwrap();
        assert!(fulfillment.cancel().is_err());
        fulfillment.deliver().unwrap();
        assert_eq!(fulfillment.status(), FulfillmentStatus::Delivered);
    }

    #[test]
    fn return_requires_authorization_and_receipt_before_completion() {
        let mut returned = Return::create(OrderId::new());

        assert!(returned.receive().is_err());
        returned.authorize().unwrap();
        returned.receive().unwrap();
        returned.complete().unwrap();
        assert_eq!(returned.status(), ReturnStatus::Completed);
    }
}
