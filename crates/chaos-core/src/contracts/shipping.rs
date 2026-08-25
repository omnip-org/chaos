use async_trait::async_trait;
use uuid::Uuid;

use crate::ApplicationError;

/// Provider-neutral shipment operation. Manual fulfillment uses the same
/// capability boundary but does not make a network call; carrier adapters can
/// implement the same port later without changing order state transitions.
pub struct ShippingCommand {
    pub operation: ShippingOperation,
    pub provider_account_id: Uuid,
    pub order_id: Uuid,
    pub fulfillment_id: Uuid,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShippingOperation {
    Create,
    Shipped,
    Delivered,
    Cancelled,
}

#[derive(Debug)]
pub struct ShippingResult {
    pub provider_reference_id: Option<String>,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
}

#[async_trait]
pub trait ShippingProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(&self, command: ShippingCommand) -> Result<ShippingResult, ApplicationError>;
}
