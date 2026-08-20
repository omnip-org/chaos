use chaos_application::{
    payments::{CreatePaymentProviderAccountInput, UpdatePaymentProviderAccountInput},
    ports::{AdminActor, PaymentProviderAccountDetail},
};
use chaos_domain::payments::PaymentProviderAccountId;
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, JsonSchema)]
pub struct ListPaymentProvidersParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetPaymentProviderParams {
    pub payment_provider_account_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreatePaymentProviderParams {
    pub provider: String,
    pub display_name: String,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdatePaymentProviderParams {
    pub payment_provider_account_id: String,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[tool_router(router = payment_providers_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(description = "List Payment Provider accounts in the selected Store.")]
    async fn list_payment_provider_accounts(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListPaymentProvidersParams>,
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
            .payment_provider_administration
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
                    .map(provider_json)
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

    #[tool(description = "Get one Payment Provider account in the selected Store.")]
    async fn get_payment_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetPaymentProviderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let id = match uuid::Uuid::parse_str(&params.payment_provider_account_id) {
            Ok(id) => PaymentProviderAccountId::from_uuid(id),
            Err(_) => return Ok(invalid_id("payment_provider_account_id")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .payment_provider_administration
            .get(actor, store_id, id)
            .await
        {
            Ok(value) => Ok(text_result(provider_json(value))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Create a Payment Provider account in the selected Store.")]
    async fn create_payment_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreatePaymentProviderParams>,
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
            .payment_provider_administration
            .create(CreatePaymentProviderAccountInput {
                actor,
                store_id,
                provider: params.provider,
                display_name: params.display_name,
                external_account_reference: params.external_account_reference,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                enabled: params.enabled,
                checked_at: self.state.clock.now(),
                idempotency,
            })
            .await
        {
            Ok(value) => Ok(text_result(provider_json(value))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Update a Payment Provider account in the selected Store.")]
    async fn update_payment_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdatePaymentProviderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let id = match uuid::Uuid::parse_str(&params.payment_provider_account_id) {
            Ok(id) => PaymentProviderAccountId::from_uuid(id),
            Err(_) => return Ok(invalid_id("payment_provider_account_id")),
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .payment_provider_administration
            .update(UpdatePaymentProviderAccountInput {
                actor,
                store_id,
                id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                enabled: params.enabled,
                checked_at: self.state.clock.now(),
                idempotency,
            })
            .await
        {
            Ok(value) => Ok(text_result(provider_json(value))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

impl ChaosMcp {
    pub(super) async fn store_actor(
        &self,
        parts: &http::request::Parts,
    ) -> Result<chaos_application::merchant::StoreActor, CallToolResult> {
        match crate::auth::authenticate_mcp(
            &self.state.mcp_key_authentication,
            &self.state.merchant_queries,
            parts,
        )
        .await?
        {
            AdminActor::Store(actor) => Ok(actor),
            AdminActor::Machine(_) => unreachable!("MCP authentication returns a User actor"),
        }
    }
}

fn invalid_id(field: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field} must be a valid UUID"),
    }))
}

fn provider_json(value: PaymentProviderAccountDetail) -> serde_json::Value {
    json!({
        "id": value.account.id().as_uuid(),
        "provider": value.account.provider(),
        "display_name": value.account.display_name(),
        "external_account_reference": value.account.external_account_reference(),
        "enabled": value.account.enabled(),
        "credentials_configured": value.credentials_configured,
        "readiness_status": value.readiness_status.as_str(),
        "readiness_checked_at": value.readiness_checked_at.map(|v| v.to_string()),
        "readiness_valid_until": value.readiness_valid_until.map(|v| v.to_string()),
        "readiness_blocker_codes": value.readiness_blocker_codes,
        "credential_rotation_expires_at": value.credential_rotation_expires_at.map(|v| v.to_string()),
        "webhook_rotation_expires_at": value.webhook_rotation_expires_at.map(|v| v.to_string()),
        "created_at": value.created_at.to_string(),
        "updated_at": value.updated_at.to_string(),
    })
}
