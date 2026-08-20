use async_trait::async_trait;
use chaos_domain::{identity::UserId, store::StoreId};
use secrecy::SecretString;

use crate::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSecretKind {
    PaymentCredential,
    PaymentWebhook,
    ShippingCredential,
    AnalyticsCredential,
    NotificationCredential,
    NotificationWebhook,
}

impl ProviderSecretKind {
    pub const fn as_path_segment(self) -> &'static str {
        match self {
            Self::PaymentCredential => "payment-credential",
            Self::PaymentWebhook => "payment-webhook",
            Self::ShippingCredential => "shipping-credential",
            Self::AnalyticsCredential => "analytics-credential",
            Self::NotificationCredential => "notification-credential",
            Self::NotificationWebhook => "notification-webhook",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "payment_credential" => Some(Self::PaymentCredential),
            "payment_webhook" => Some(Self::PaymentWebhook),
            "shipping_credential" => Some(Self::ShippingCredential),
            "analytics_credential" => Some(Self::AnalyticsCredential),
            "notification_credential" => Some(Self::NotificationCredential),
            "notification_webhook" => Some(Self::NotificationWebhook),
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
