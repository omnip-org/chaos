use async_trait::async_trait;
use chaos_domain::{identity::UserId, store::StoreId};
use secrecy::SecretString;

use crate::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSecretKind {
    PaymentCredential,
    PaymentWebhook,
    EmailCredential,
    EmailWebhook,
    ShippingCredential,
    ShippingWebhook,
    AnalyticsCredential,
}

impl ProviderSecretKind {
    pub const fn as_path_segment(self) -> &'static str {
        match self {
            Self::PaymentCredential => "payment-credential",
            Self::PaymentWebhook => "payment-webhook",
            Self::EmailCredential => "email-credential",
            Self::EmailWebhook => "email-webhook",
            Self::ShippingCredential => "shipping-credential",
            Self::ShippingWebhook => "shipping-webhook",
            Self::AnalyticsCredential => "analytics-credential",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "payment_credential" => Some(Self::PaymentCredential),
            "payment_webhook" => Some(Self::PaymentWebhook),
            "email_credential" => Some(Self::EmailCredential),
            "email_webhook" => Some(Self::EmailWebhook),
            "shipping_credential" => Some(Self::ShippingCredential),
            "shipping_webhook" => Some(Self::ShippingWebhook),
            "analytics_credential" => Some(Self::AnalyticsCredential),
            _ => None,
        }
    }
}

#[async_trait]
pub trait ProviderSecretWriter: Send + Sync {
    async fn create(
        &self,
        store_id: StoreId,
        created_by: UserId,
        kind: ProviderSecretKind,
        value: &SecretString,
    ) -> Result<String, ApplicationError>;
}

/// Shared secret resolution port used by capability adapters. The reference
/// remains opaque to the adapter and can point to encrypted storage or a
/// tightly scoped environment variable.
#[async_trait]
pub trait IntegrationSecretResolver: Send + Sync {
    async fn resolve(&self, reference: &str) -> Result<SecretString, ApplicationError>;
}
