use uuid::Uuid;

use crate::{DomainError, FieldViolation, sales::OrderId};

macro_rules! fulfillment_id {
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

fulfillment_id!(FulfillmentId);
fulfillment_id!(ShippingProviderAccountId);

/// Shared shipping vocabulary for Fulfillment rows and the Order's shipping
/// projection. `Pending` is used only when an Order has no active shipment;
/// an individual Fulfillment starts at `AwaitingPickup`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FulfillmentStatus {
    Pending,
    AwaitingPickup,
    Shipped,
    Delivered,
    Cancelled,
}

impl FulfillmentStatus {
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
pub struct Fulfillment {
    id: FulfillmentId,
    order_id: OrderId,
    shipping_provider_account_id: ShippingProviderAccountId,
    status: FulfillmentStatus,
    tracking_number: Option<String>,
    tracking_url: Option<String>,
}

impl Fulfillment {
    pub fn create(
        order_id: OrderId,
        shipping_provider_account_id: ShippingProviderAccountId,
        tracking_number: Option<String>,
        tracking_url: Option<String>,
    ) -> Result<Self, DomainError> {
        validate_tracking(tracking_number.as_deref(), tracking_url.as_deref())?;
        Ok(Self {
            id: FulfillmentId::new(),
            order_id,
            shipping_provider_account_id,
            status: FulfillmentStatus::AwaitingPickup,
            tracking_number,
            tracking_url,
        })
    }

    pub fn rehydrate(
        id: FulfillmentId,
        order_id: OrderId,
        shipping_provider_account_id: ShippingProviderAccountId,
        status: FulfillmentStatus,
        tracking_number: Option<String>,
        tracking_url: Option<String>,
    ) -> Self {
        Self {
            id,
            order_id,
            shipping_provider_account_id,
            status,
            tracking_number,
            tracking_url,
        }
    }

    pub const fn id(&self) -> FulfillmentId {
        self.id
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn shipping_provider_account_id(&self) -> ShippingProviderAccountId {
        self.shipping_provider_account_id
    }

    pub const fn status(&self) -> FulfillmentStatus {
        self.status
    }

    pub fn tracking_number(&self) -> Option<&str> {
        self.tracking_number.as_deref()
    }

    pub fn tracking_url(&self) -> Option<&str> {
        self.tracking_url.as_deref()
    }

    pub fn mark_shipped(
        &mut self,
        tracking_number: Option<String>,
        tracking_url: Option<String>,
    ) -> Result<bool, DomainError> {
        if let Some(number) = tracking_number {
            validate_tracking(Some(&number), None)?;
            self.tracking_number = Some(number);
        }
        if let Some(url) = tracking_url {
            validate_tracking(None, Some(&url))?;
            self.tracking_url = Some(url);
        }
        self.advance(FulfillmentStatus::Shipped)
    }

    pub fn mark_delivered(&mut self) -> Result<bool, DomainError> {
        self.advance(FulfillmentStatus::Delivered)
    }

    pub fn cancel(&mut self) -> Result<bool, DomainError> {
        self.advance(FulfillmentStatus::Cancelled)
    }

    fn advance(&mut self, target: FulfillmentStatus) -> Result<bool, DomainError> {
        if self.status == target {
            return Ok(false);
        }
        let allowed = matches!(
            (self.status, target),
            (
                FulfillmentStatus::AwaitingPickup,
                FulfillmentStatus::Shipped
            ) | (
                FulfillmentStatus::AwaitingPickup,
                FulfillmentStatus::Cancelled
            ) | (FulfillmentStatus::Shipped, FulfillmentStatus::Delivered)
                | (FulfillmentStatus::Shipped, FulfillmentStatus::Cancelled)
        );
        if !allowed {
            return Err(invalid_transition(self.status.as_str(), target.as_str()));
        }
        self.status = target;
        Ok(true)
    }
}

fn validate_tracking(number: Option<&str>, url: Option<&str>) -> Result<(), DomainError> {
    if let Some(number) = number
        && (number.trim().is_empty() || number.chars().count() > 255)
    {
        return Err(validation(
            "tracking_number",
            "must contain 1-255 characters",
        ));
    }
    if let Some(url) = url
        && (!url.starts_with("https://") || url.chars().count() > 2048)
    {
        return Err(validation(
            "tracking_url",
            "must be an https URL of at most 2048 characters",
        ));
    }
    Ok(())
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

fn invalid_transition(from: &str, to: &str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field: "status",
        reason: format!("cannot transition from {from} to {to}"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fulfillment_progresses_awaiting_pickup_to_shipped_to_delivered() {
        let mut fulfillment =
            Fulfillment::create(OrderId::new(), ShippingProviderAccountId::new(), None, None)
                .unwrap();
        assert_eq!(fulfillment.status(), FulfillmentStatus::AwaitingPickup);
        assert!(fulfillment.mark_delivered().is_err());
        assert!(
            fulfillment
                .mark_shipped(
                    Some("1Z999".into()),
                    Some("https://track.example/1Z999".into())
                )
                .unwrap()
        );
        assert_eq!(fulfillment.tracking_number(), Some("1Z999"));
        assert!(fulfillment.mark_delivered().unwrap());
        assert_eq!(fulfillment.status(), FulfillmentStatus::Delivered);
        assert!(fulfillment.cancel().is_err());
    }

    #[test]
    fn fulfillment_can_be_cancelled_before_delivery_but_not_after() {
        let mut fulfillment =
            Fulfillment::create(OrderId::new(), ShippingProviderAccountId::new(), None, None)
                .unwrap();
        assert!(fulfillment.cancel().unwrap());
        assert!(fulfillment.mark_shipped(None, None).is_err());
    }

    #[test]
    fn fulfillment_rejects_a_non_https_tracking_url() {
        let result = Fulfillment::create(
            OrderId::new(),
            ShippingProviderAccountId::new(),
            None,
            Some("http://track.example/1Z999".into()),
        );
        assert!(result.is_err());
    }
}
