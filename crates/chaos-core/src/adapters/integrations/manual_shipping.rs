use async_trait::async_trait;

use crate::{
    ApplicationError,
    contracts::{ShippingCommand, ShippingProvider, ShippingResult},
};

/// Manual fulfillment is an explicit shipping adapter. It records state in
/// Commerce and deliberately performs no network call.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManualShippingProvider;

#[async_trait]
impl ShippingProvider for ManualShippingProvider {
    fn name(&self) -> &'static str {
        "manual"
    }

    async fn execute(&self, command: ShippingCommand) -> Result<ShippingResult, ApplicationError> {
        Ok(ShippingResult {
            provider_reference_id: None,
            tracking_number: command.tracking_number,
            tracking_url: command.tracking_url,
        })
    }
}
