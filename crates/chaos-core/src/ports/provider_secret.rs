use async_trait::async_trait;
use chaos_domain::{identity::UserId, store::StoreId};
use secrecy::SecretString;

use crate::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSecretKind {
    PaymentCredential,
    PaymentWebhook,
    AnalyticsCredential,
}

impl ProviderSecretKind {
    pub const fn as_path_segment(self) -> &'static str {
        match self {
            Self::PaymentCredential => "payment-credential",
            Self::PaymentWebhook => "payment-webhook",
            Self::AnalyticsCredential => "analytics-credential",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "payment_credential" => Some(Self::PaymentCredential),
            "payment_webhook" => Some(Self::PaymentWebhook),
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
