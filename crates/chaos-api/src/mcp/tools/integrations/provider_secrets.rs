use chaos_core::store::CreateProviderSecretInput;
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSecretKindParam {
    PaymentCredential,
    PaymentWebhook,
    EmailCredential,
    EmailWebhook,
    ShippingCredential,
    ShippingWebhook,
    AnalyticsCredential,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateProviderSecretParams {
    /// The Store UUID where the encrypted secret will be stored.
    pub store_id: String,
    /// Secret purpose. Payment uses `payment_credential` and `payment_webhook`; Email uses `email_credential` and `email_webhook`; Shipping uses `shipping_credential` and `shipping_webhook`; Analytics uses `analytics_credential`.
    pub kind: ProviderSecretKindParam,
    /// The secret value to store. It is encrypted immediately and only an opaque `enc://...` reference is returned.
    /// Returned as an opaque reference, never in plaintext, from any read path.
    pub value: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = provider_secrets_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Store a provider secret (payment/email/shipping/analytics credential or webhook secret) in the \
                        selected Store. For Stripe, payment_credential must be JSON with \
                        secret_key and publishable_key, while payment_webhook must be the raw \
                        Stripe whsec_... endpoint signing secret. For Resend, email_credential is the \
                        raw Resend API key and email_webhook is the raw Resend signing secret. The value \
                        is encrypted at rest and referenced by an opaque string thereafter; it cannot be \
                        read back. Calling it twice creates two independent secret references. There is no \
                        update or delete tool for provider secrets; create a new one to rotate. Requires \
                        confirm: true."
    )]
    async fn create_provider_secret(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateProviderSecretParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            &parts,
            &params.store_id,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let kind = match params.kind {
            ProviderSecretKindParam::PaymentCredential => {
                chaos_core::contracts::ProviderSecretKind::PaymentCredential
            }
            ProviderSecretKindParam::PaymentWebhook => {
                chaos_core::contracts::ProviderSecretKind::PaymentWebhook
            }
            ProviderSecretKindParam::EmailCredential => {
                chaos_core::contracts::ProviderSecretKind::EmailCredential
            }
            ProviderSecretKindParam::EmailWebhook => {
                chaos_core::contracts::ProviderSecretKind::EmailWebhook
            }
            ProviderSecretKindParam::ShippingCredential => {
                chaos_core::contracts::ProviderSecretKind::ShippingCredential
            }
            ProviderSecretKindParam::ShippingWebhook => {
                chaos_core::contracts::ProviderSecretKind::ShippingWebhook
            }
            ProviderSecretKindParam::AnalyticsCredential => {
                chaos_core::contracts::ProviderSecretKind::AnalyticsCredential
            }
        };
        let store_id = actor.store_id();

        match self
            .state
            .provider_secret_management
            .create(CreateProviderSecretInput {
                actor,
                store_id,
                kind,
                value: SecretString::from(params.value),
            })
            .await
        {
            Ok(secret_reference) => {
                Ok(text_result(json!({ "secret_reference": secret_reference })))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }
}
