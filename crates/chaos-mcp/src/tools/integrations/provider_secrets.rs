use chaos_application::store::CreateProviderSecretInput;
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

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateProviderSecretParams {
    /// Secret purpose. For Stripe, use `payment_credential` for a JSON object containing `secret_key` and `publishable_key`, and `payment_webhook` for the raw `whsec_...` signing secret. Other supported kinds are `shipping_credential` and `analytics_credential`.
    pub kind: String,
    /// The secret value to store. It is encrypted immediately and only an opaque `enc://...` reference is returned.
    /// Returned as an opaque reference, never in plaintext, from any read path.
    pub value: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = provider_secrets_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Store a provider secret (payment/shipping/analytics credential) in the \
                        selected Store. For Stripe, payment_credential must be JSON with \
                        secret_key and publishable_key, while payment_webhook must be the raw \
                        Stripe whsec_... endpoint signing secret. The value is encrypted at rest and \
                        referenced by an opaque string thereafter; it cannot be read back. \
                        This tool has no idempotency_key parameter because the underlying \
                        operation is not idempotent — calling it twice creates two independent \
                        secret references. There is no update or \
                        delete tool for provider secrets; create a new one to rotate. Requires \
                        confirm: true."
    )]
    async fn create_provider_secret(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateProviderSecretParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            &parts,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let kind = match chaos_application::ports::ProviderSecretKind::parse(&params.kind) {
            Some(kind) => kind,
            None => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "kind must be one of: payment_credential, payment_webhook, \
                                shipping_credential, analytics_credential",
                })));
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
