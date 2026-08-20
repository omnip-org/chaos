use chaos_application::notifications::ConfigureNotificationProviderInput;
use chaos_application::ports::{AdminActor, NotificationProviderAccountDetail};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
    tools::ChaosMcp,
};

#[derive(Deserialize, JsonSchema)]
pub struct ListNotificationProvidersParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureNotificationProviderParams {
    /// Currently only `resend` is supported.
    pub provider: String,
    pub display_name: String,
    /// A verified Resend sender, for example `Store <orders@example.com>`.
    pub sender: String,
    /// Opaque reference returned by create_provider_secret with kind notification_credential.
    pub credential_secret_reference: String,
    /// Opaque reference returned by create_provider_secret with kind notification_webhook.
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[tool_router(router = notification_providers_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(description = "List Notification Provider accounts in the selected Store.")]
    async fn list_notification_provider_accounts(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<ListNotificationProvidersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .notification_provider_administration
            .list(AdminActor::Store(actor), store_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(provider_json).collect::<Vec<_>>()
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create or replace the selected Store's Resend notification configuration. Store the API key and webhook signing secret with create_provider_secret first. Requires Store Owner access and confirm: true."
    )]
    async fn configure_notification_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureNotificationProviderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .notification_provider_administration
            .configure(ConfigureNotificationProviderInput {
                actor: AdminActor::Store(actor),
                store_id,
                provider: params.provider,
                display_name: params.display_name,
                sender: params.sender,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                enabled: params.enabled,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(provider_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn provider_json(detail: NotificationProviderAccountDetail) -> serde_json::Value {
    let id = detail.account.id().as_uuid();
    json!({
        "id": id,
        "provider": detail.account.provider(),
        "display_name": detail.account.display_name(),
        "sender": detail.account.sender(),
        "enabled": detail.account.enabled(),
        "credentials_configured": detail.credentials_configured,
        "webhook_configured": detail.webhook_configured,
        "webhook_path": format!("/webhooks/v1/notifications/{}/{}", detail.account.provider(), id),
        "created_at": detail.created_at.to_string(),
        "updated_at": detail.updated_at.to_string(),
    })
}
