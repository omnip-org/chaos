use chaos_core::{
    contracts::{
        EmailAccountConfiguration, EmailBrandDetail, EmailProviderAccountDetail,
        EmailProviderAccountPage,
    },
    email::{
        ConfigureEmailBrandInput, CreateEmailProviderAccountInput, ResetEmailBrandInput,
        UpdateEmailProviderAccountInput,
    },
};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
    tools::ChaosMcp,
};

#[derive(Deserialize, JsonSchema)]
pub struct ListEmailAccountsParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetEmailAccountParams {
    /// The Store UUID containing the account.
    pub store_id: String,
    /// The Email Provider Account UUID.
    pub email_account_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateEmailAccountParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// Human-readable name for this Email Provider Account.
    pub display_name: String,
    /// Opaque reference returned by `create_provider_secret` with kind `email_credential`.
    /// Never pass a Resend API key directly.
    pub credential_secret_reference: String,
    /// Optional opaque reference returned by `create_provider_secret` with kind `email_webhook`.
    /// Provide this to accept Resend delivery webhooks.
    pub webhook_secret_reference: Option<String>,
    /// Verified sender email address configured in Resend.
    pub from_email: String,
    /// Optional sender display name.
    pub from_name: Option<String>,
    /// Whether this account may send transactional email immediately.
    pub enabled: bool,
    /// Must be explicitly set to true. This action affects live Store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateEmailAccountParams {
    /// The Store UUID containing the account.
    pub store_id: String,
    /// The Email Provider Account UUID.
    pub email_account_id: String,
    pub display_name: String,
    /// Opaque reference returned by `create_provider_secret` with kind `email_credential`.
    /// Never pass a Resend API key directly.
    pub credential_secret_reference: String,
    /// Optional opaque reference returned by `create_provider_secret` with kind `email_webhook`.
    /// Set to null to disable delivery webhooks for this account.
    pub webhook_secret_reference: Option<String>,
    /// Verified sender email address configured in Resend.
    pub from_email: String,
    /// Optional sender display name.
    pub from_name: Option<String>,
    pub enabled: bool,
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetEmailBrandParams {
    /// The Store UUID whose effective Email brand settings should be inspected.
    pub store_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureEmailBrandParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// Optional display name. Null uses the Store name.
    pub brand_name: Option<String>,
    /// Public HTTPS URL for the brand logo. A ready Media asset's public_url may be used.
    pub logo_url: Option<String>,
    /// Main action and link color, for example #175CD3.
    pub primary_color: String,
    /// Secondary border and accent color, for example #0E7490.
    pub accent_color: String,
    /// Outer email background color.
    pub background_color: String,
    /// Email card background color.
    pub surface_color: String,
    /// Main text color.
    pub text_color: String,
    /// Muted text color.
    pub muted_text_color: String,
    /// Optional customer support email address.
    pub support_email: Option<String>,
    /// Optional public HTTPS customer support URL.
    pub support_url: Option<String>,
    /// Must be explicitly set to true. This action changes live Store data.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResetEmailBrandParams {
    /// The Store UUID whose brand override should be removed.
    pub store_id: String,
    /// Must be explicitly set to true. This action changes live Store data.
    pub confirm: bool,
}

#[tool_router(router = email_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List Email Provider Accounts in the selected Store. Credentials are never returned. The current implementation supports Resend and exposes only non-sensitive configuration state."
    )]
    async fn list_email_accounts(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListEmailAccountsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let after = match params.cursor.as_deref().map(Uuid::parse_str) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => return Ok(invalid_id("cursor")),
            None => None,
        };
        let limit = params.limit.unwrap_or(20).clamp(1, 100);
        let store_id = actor.store_id();
        match self
            .state
            .email_provider_account_administration
            .list(actor, store_id, after, limit)
            .await
        {
            Ok(page) => Ok(text_result(email_accounts_json(
                page,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get one Email Provider Account in the selected Store. Credentials are never returned."
    )]
    async fn get_email_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetEmailAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let id = match Uuid::parse_str(&params.email_account_id) {
            Ok(id) => id,
            Err(_) => return Ok(invalid_id("email_account_id")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .email_provider_account_administration
            .get(actor, store_id, id)
            .await
        {
            Ok(account) => Ok(text_result(email_account_json(
                account,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create the selected Store's Resend Email Provider Account. First call create_provider_secret with kind email_credential and optionally email_webhook, then pass the returned opaque enc:// references. The from_email must be a sender address verified in Resend. Credentials are never returned. Requires Owner role and confirm: true."
    )]
    async fn create_email_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateEmailAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        match self
            .state
            .email_provider_account_administration
            .create(CreateEmailProviderAccountInput {
                actor,
                store_id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                configuration: EmailAccountConfiguration {
                    from_email: params.from_email,
                    from_name: params.from_name,
                },
                enabled: params.enabled,
            })
            .await
        {
            Ok(account) => Ok(text_result(email_account_json(
                account,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update the selected Store's Resend Email Provider Account. Pass new opaque secret references when rotating credentials, and pass null for webhook_secret_reference to disable Resend delivery webhooks. The from_email must be a sender address verified in Resend. Credentials are never returned. Requires Owner role and confirm: true."
    )]
    async fn update_email_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateEmailAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let id = match Uuid::parse_str(&params.email_account_id) {
            Ok(id) => id,
            Err(_) => return Ok(invalid_id("email_account_id")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .email_provider_account_administration
            .update(UpdateEmailProviderAccountInput {
                actor,
                store_id,
                id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                webhook_secret_reference: params.webhook_secret_reference,
                configuration: EmailAccountConfiguration {
                    from_email: params.from_email,
                    from_name: params.from_name,
                },
                enabled: params.enabled,
            })
            .await
        {
            Ok(account) => Ok(text_result(email_account_json(
                account,
                &self.state.public_base_url,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get the selected Store's effective Email brand settings. Transactional Email templates remain global and server-owned: order data, product lines, totals, and tracking links are rendered by Chaos."
    )]
    async fn get_email_brand(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetEmailBrandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .email_brand_administration
            .get(actor, store_id)
            .await
        {
            Ok(brand) => Ok(text_result(email_brand_json(brand))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Configure the selected Store's Email brand tokens in the database. Set brand_name, logo_url, colors, and optional support contacts; the global server-owned template will use them while Chaos continues to render product information, totals, and tracking links. logo_url and support_url must be public HTTPS URLs. Requires Owner role and confirm: true."
    )]
    async fn configure_email_brand(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureEmailBrandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        match self
            .state
            .email_brand_administration
            .configure(ConfigureEmailBrandInput {
                actor,
                store_id,
                brand_name: params.brand_name,
                logo_url: params.logo_url,
                primary_color: params.primary_color,
                accent_color: params.accent_color,
                background_color: params.background_color,
                surface_color: params.surface_color,
                text_color: params.text_color,
                muted_text_color: params.muted_text_color,
                support_email: params.support_email,
                support_url: params.support_url,
            })
            .await
        {
            Ok(brand) => Ok(text_result(email_brand_json(brand))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Remove the selected Store's Email brand override and restore the platform defaults, including the Store name as the brand name. The global transactional template remains active. Requires Owner role and confirm: true."
    )]
    async fn reset_email_brand(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ResetEmailBrandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        match self
            .state
            .email_brand_administration
            .reset(ResetEmailBrandInput { actor, store_id })
            .await
        {
            Ok(brand) => Ok(text_result(email_brand_json(brand))),
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

fn email_accounts_json(page: EmailProviderAccountPage, public_base_url: &str) -> serde_json::Value {
    let next_cursor = page
        .has_more
        .then(|| page.items.last().map(|account| account.id))
        .flatten();
    json!({
        "items": page.items.into_iter().map(|account| email_account_json(account, public_base_url)).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_cursor": next_cursor,
    })
}

fn email_account_json(
    account: EmailProviderAccountDetail,
    public_base_url: &str,
) -> serde_json::Value {
    let webhook_url = format!(
        "{}/webhooks/v1/email/{}/{}",
        public_base_url.trim_end_matches('/'),
        account.provider,
        account.id
    );
    json!({
        "id": account.id,
        "account_type": "email_provider",
        "provider": account.provider,
        "display_name": account.display_name,
        "enabled": account.enabled,
        "credentials_configured": account.credentials_configured,
        "webhook_configured": account.webhook_configured,
        "sender": {
            "email": account.configuration.from_email,
            "name": account.configuration.from_name,
        },
        "email_setup": {
            "webhook_url": webhook_url,
            "signature_headers": ["svix-id", "svix-timestamp", "svix-signature"],
            "events_to_receive": [
                "email.sent",
                "email.delivered",
                "email.delivery_delayed",
                "email.bounced",
                "email.complained",
                "email.suppressed"
            ]
        },
        "created_at": account.created_at.to_string(),
        "updated_at": account.updated_at.to_string(),
    })
}

fn email_brand_json(brand: EmailBrandDetail) -> serde_json::Value {
    let configuration = brand.configuration;
    json!({
        "customized": brand.customized,
        "brand": {
            "name": configuration.brand_name,
            "logo_url": configuration.logo_url,
            "primary_color": configuration.primary_color,
            "accent_color": configuration.accent_color,
            "background_color": configuration.background_color,
            "surface_color": configuration.surface_color,
            "text_color": configuration.text_color,
            "muted_text_color": configuration.muted_text_color,
            "support_email": configuration.support_email,
            "support_url": configuration.support_url,
        },
        "template_policy": {
            "scope": "global",
            "owner": "chaos",
            "brand_storage": "integration.provider_accounts.configuration.brand",
            "template_keys": ["order_confirmation"],
            "server_rendered_data": [
                "order_number",
                "line_items",
                "total_amount",
                "currency",
                "tracking_url"
            ],
            "customizable_by_store": [
                "brand_name",
                "logo_url",
                "primary_color",
                "accent_color",
                "background_color",
                "surface_color",
                "text_color",
                "muted_text_color",
                "support_email",
                "support_url"
            ]
        },
    })
}
