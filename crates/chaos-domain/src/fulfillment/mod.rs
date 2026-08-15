use uuid::Uuid;

use crate::{
    CurrencyCode, DomainError, FieldViolation, catalog::ProductVariantId, pricing::Money,
    sales::OrderId,
};

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
operation_id!(ShippingServiceId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShippingServiceStatus {
    Active,
    Archived,
}

impl ShippingServiceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingService {
    id: ShippingServiceId,
    code: String,
    name: String,
    rate: Money,
    estimated_min_days: u16,
    estimated_max_days: u16,
    destination_countries: Vec<String>,
    status: ShippingServiceStatus,
}

impl ShippingService {
    pub fn create(
        code: impl Into<String>,
        name: impl Into<String>,
        rate: Money,
        estimated_min_days: u16,
        estimated_max_days: u16,
        destination_countries: Vec<String>,
    ) -> Result<Self, DomainError> {
        Self::build(
            ShippingServiceId::new(),
            code,
            name,
            rate,
            estimated_min_days,
            estimated_max_days,
            destination_countries,
            ShippingServiceStatus::Active,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rehydrate(
        id: ShippingServiceId,
        code: impl Into<String>,
        name: impl Into<String>,
        rate: Money,
        estimated_min_days: u16,
        estimated_max_days: u16,
        destination_countries: Vec<String>,
        status: ShippingServiceStatus,
    ) -> Result<Self, DomainError> {
        Self::build(
            id,
            code,
            name,
            rate,
            estimated_min_days,
            estimated_max_days,
            destination_countries,
            status,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        id: ShippingServiceId,
        code: impl Into<String>,
        name: impl Into<String>,
        rate: Money,
        estimated_min_days: u16,
        estimated_max_days: u16,
        mut destination_countries: Vec<String>,
        status: ShippingServiceStatus,
    ) -> Result<Self, DomainError> {
        let code = code.into();
        let name = name.into();
        if code.is_empty()
            || code.len() > 64
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(validation(
                "code",
                "must contain 1-64 lowercase letters, digits, or hyphens",
            ));
        }
        validate_text("name", &name, 120)?;
        if rate.amount_minor() < 0 {
            return Err(validation("amount_minor", "must be zero or greater"));
        }
        if estimated_min_days > estimated_max_days || estimated_max_days > 365 {
            return Err(validation(
                "estimated_delivery_days",
                "must be ordered and no greater than 365 days",
            ));
        }
        if destination_countries.is_empty() {
            return Err(validation(
                "destination_countries",
                "must contain at least one country",
            ));
        }
        for country in &mut destination_countries {
            *country = country.trim().to_ascii_uppercase();
            if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
                return Err(validation(
                    "destination_countries",
                    "must contain ISO 3166-1 alpha-2 country codes",
                ));
            }
        }
        destination_countries.sort();
        destination_countries.dedup();
        Ok(Self {
            id,
            code,
            name,
            rate,
            estimated_min_days,
            estimated_max_days,
            destination_countries,
            status,
        })
    }

    pub const fn id(&self) -> ShippingServiceId {
        self.id
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn rate(&self) -> &Money {
        &self.rate
    }
    pub const fn estimated_min_days(&self) -> u16 {
        self.estimated_min_days
    }
    pub const fn estimated_max_days(&self) -> u16 {
        self.estimated_max_days
    }
    pub fn destination_countries(&self) -> &[String] {
        &self.destination_countries
    }
    pub const fn status(&self) -> ShippingServiceStatus {
        self.status
    }

    pub fn serves(&self, currency: CurrencyCode, country_code: &str) -> bool {
        self.status == ShippingServiceStatus::Active
            && self.rate.currency() == currency
            && self
                .destination_countries
                .binary_search(&country_code.to_ascii_uppercase())
                .is_ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippingSelection {
    service_id: ShippingServiceId,
    code: String,
    name: String,
    amount: Money,
    estimated_min_days: u16,
    estimated_max_days: u16,
}

impl ShippingSelection {
    pub fn from_service(service: &ShippingService) -> Result<Self, DomainError> {
        if service.status() != ShippingServiceStatus::Active {
            return Err(validation(
                "shipping_service_id",
                "must reference an active service",
            ));
        }
        Ok(Self {
            service_id: service.id(),
            code: service.code().into(),
            name: service.name().into(),
            amount: service.rate().clone(),
            estimated_min_days: service.estimated_min_days(),
            estimated_max_days: service.estimated_max_days(),
        })
    }

    pub fn rehydrate(
        service_id: ShippingServiceId,
        code: impl Into<String>,
        name: impl Into<String>,
        amount: Money,
        estimated_min_days: u16,
        estimated_max_days: u16,
    ) -> Result<Self, DomainError> {
        let code = code.into();
        let name = name.into();
        if code.is_empty() || code.len() > 64 {
            return Err(validation(
                "shipping_code",
                "must contain valid snapshot text",
            ));
        }
        validate_text("shipping_name", &name, 120)?;
        if amount.amount_minor() < 0 || estimated_min_days > estimated_max_days {
            return Err(validation("shipping_selection", "contains invalid values"));
        }
        Ok(Self {
            service_id,
            code,
            name,
            amount,
            estimated_min_days,
            estimated_max_days,
        })
    }

    pub const fn service_id(&self) -> ShippingServiceId {
        self.service_id
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn amount(&self) -> &Money {
        &self.amount
    }
    pub const fn estimated_min_days(&self) -> u16 {
        self.estimated_min_days
    }
    pub const fn estimated_max_days(&self) -> u16 {
        self.estimated_max_days
    }
}

fn validate_text(field: &'static str, value: &str, max: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.chars().count() > max || value.chars().any(char::is_control)
    {
        Err(validation(
            field,
            "must contain printable text within the allowed length",
        ))
    } else {
        Ok(())
    }
}

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

    #[test]
    fn shipping_service_normalizes_regions_and_matches_currency() {
        let service = ShippingService::create(
            "express",
            "Express",
            Money::new(1_500, CurrencyCode::USD),
            1,
            2,
            vec!["us".into(), "CA".into(), "US".into()],
        )
        .unwrap();

        assert_eq!(service.destination_countries(), &["CA", "US"]);
        assert!(service.serves(CurrencyCode::USD, "us"));
        assert!(!service.serves(CurrencyCode::parse("EUR").unwrap(), "US"));
    }

    #[test]
    fn shipping_service_rejects_invalid_estimates_and_regions() {
        assert!(
            ShippingService::create(
                "standard",
                "Standard",
                Money::new(500, CurrencyCode::USD),
                5,
                2,
                vec!["US".into()],
            )
            .is_err()
        );
        assert!(
            ShippingService::create(
                "standard",
                "Standard",
                Money::new(500, CurrencyCode::USD),
                2,
                5,
                vec![],
            )
            .is_err()
        );
    }
}
