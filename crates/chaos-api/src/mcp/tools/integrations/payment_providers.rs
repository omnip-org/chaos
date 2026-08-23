use chaos_core::{
    ports::StripeAccountDetail,
    stripe::{CreateStripeAccountInput, UpdateStripeAccountInput},
};
use chaos_domain::{store::StoreId, stripe::StripeAccountId};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, JsonSchema)]
pub struct ListStripeAccountsParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetStripeAccountParams {
    pub stripe_account_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateStripeAccountParams {
    /// Display name for the store's direct Stripe account.
    pub display_name: String,
    /// Opaque reference returned by `create_provider_secret` with kind `payment_credential`. The stored value must be JSON containing `secret_key` and `publishable_key`.
    pub credential_secret_reference: String,
    /// Opaque reference returned by `create_provider_secret` with kind `payment_webhook`, containing the Stripe endpoint signing secret (`whsec_...`).
    pub webhook_secret_reference: String,
    /// Set true only after the Stripe keys and webhook endpoint are ready. A failed readiness check keeps the account disabled.
    pub enabled: bool,
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateStripeAccountParams {
    pub stripe_account_id: String,
    pub display_name: String,
    /// Opaque reference returned by `create_provider_secret` with kind `payment_credential`.
    pub credential_secret_reference: String,
    /// Opaque reference returned by `create_provider_secret` with kind `payment_webhook`.
    pub webhook_secret_reference: String,
    /// Set true only after the Stripe keys and webhook endpoint are ready. A failed readiness check keeps the account disabled.
    pub enabled: bool,
    pub confirm: bool,
}

#[tool_router(router = payment_providers_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(description = "List Stripe accounts in the selected Store.")]
    async fn list_stripe_accounts(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListStripeAccountsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let after = match params.cursor.as_deref().map(uuid::Uuid::parse_str) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => return Ok(invalid_id("cursor")),
            None => None,
        };
        let store_id = actor.store_id();
        match self
            .state
            .stripe_account_administration
            .list(
                actor,
                store_id,
                after,
                params.limit.unwrap_or(20).clamp(1, 100),
            )
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|value| stripe_account_json(value, store_id, &self.state.public_base_url))
                    .collect::<Vec<_>>();
                let next_cursor = page
                    .has_more
                    .then(|| {
                        items
                            .last()
                            .and_then(|item| item["id"].as_str().map(String::from))
                    })
                    .flatten();
                Ok(text_result(json!({
                    "items": items,
                    "has_more": page.has_more,
                    "next_cursor": next_cursor,
                })))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Get one Stripe account in the selected Store.")]
    async fn get_stripe_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetStripeAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let id = match uuid::Uuid::parse_str(&params.stripe_account_id) {
            Ok(id) => StripeAccountId::from_uuid(id),
            Err(_) => return Ok(invalid_id("stripe_account_id")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .stripe_account_administration
            .get(actor, store_id, id)
            .await
        {
            Ok(value) => Ok(text_result(stripe_account_json(
                value,
                store_id,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create and readiness-check the selected Store's direct Stripe account for Embedded Checkout. Stripe Connect and Stripe-Account headers are not used. Store Stripe credentials with create_provider_secret first. The result includes the exact per-Store Webhook Endpoint URL and the events to enable in Stripe Dashboard. If readiness fails, the account remains disabled. Requires confirmation."
    )]
    async fn create_stripe_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateStripeAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        match self
            .state
            .stripe_account_administration
            .create(CreateStripeAccountInput {
                actor,
                store_id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                enabled: params.enabled,
                checked_at: self.state.clock.now(),
            })
            .await
        {
            Ok(value) => Ok(text_result(stripe_account_json(
                value,
                store_id,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update and re-check the selected Store's direct Stripe Embedded Checkout account. Stripe Connect, Stripe-Account headers, and platform labels are not supported. The credential and webhook values must be opaque secret references, not plaintext keys. A configured account is not necessarily enabled: inspect readiness_status and readiness_blocker_codes before using it. The exact per-Store Webhook Endpoint URL and required Stripe events are returned in the result. Requires confirmation."
    )]
    async fn update_stripe_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateStripeAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let id = match uuid::Uuid::parse_str(&params.stripe_account_id) {
            Ok(id) => StripeAccountId::from_uuid(id),
            Err(_) => return Ok(invalid_id("stripe_account_id")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .stripe_account_administration
            .update(UpdateStripeAccountInput {
                actor,
                store_id,
                id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                enabled: params.enabled,
                checked_at: self.state.clock.now(),
            })
            .await
        {
            Ok(value) => Ok(text_result(stripe_account_json(
                value,
                store_id,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn invalid_id(field: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field} must be a valid UUID"),
    }))
}

fn stripe_account_json(
    value: StripeAccountDetail,
    store_id: StoreId,
    public_base_url: &str,
) -> serde_json::Value {
    let id = value.account.id().as_uuid();
    let webhook_path = format!("/storefront/v1/webhooks/stripe/{}", store_id.as_uuid());
    let webhook_url = format!(
        "{}/{}",
        public_base_url.trim_end_matches('/'),
        webhook_path.trim_start_matches('/')
    );
    json!({
        "id": id,
        "account_type": "direct_stripe_account",
        "display_name": value.account.display_name(),
        "enabled": value.account.enabled(),
        "credentials_configured": value.credentials_configured,
        "readiness_status": value.readiness_status.as_str(),
        "readiness_checked_at": value.readiness_checked_at.map(|v| v.to_string()),
        "readiness_valid_until": value.readiness_valid_until.map(|v| v.to_string()),
        "readiness_blocker_codes": value.readiness_blocker_codes,
        "credential_rotation_expires_at": value.credential_rotation_expires_at.map(|v| v.to_string()),
        "webhook_rotation_expires_at": value.webhook_rotation_expires_at.map(|v| v.to_string()),
        "stripe_setup": {
            "account_model": "direct_stripe_account_using_the_configured_api_keys",
            "webhook_url": webhook_url,
            "signature_header": "Stripe-Signature",
            "events_to_enable": [
                "checkout.session.completed",
                "checkout.session.async_payment_succeeded",
                "checkout.session.async_payment_failed",
                "checkout.session.expired",
                "refund.created",
                "refund.updated"
            ],
            "signing_secret": "Use the whsec_... signing secret from Stripe Dashboard → Developers → Webhooks → this endpoint",
            "mode": "Use Stripe Test mode with sk_test_/pk_test_ keys and Live mode with sk_live_/pk_live_ keys"
        },
        "created_at": value.created_at.to_string(),
        "updated_at": value.updated_at.to_string(),
    })
}
