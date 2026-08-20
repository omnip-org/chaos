use chaos_application::{
    ports::TaxRuleDetail,
    pricing::{ChangeTaxRuleStatusInput, CreateTaxRuleInput},
};
use chaos_domain::{pricing::TaxRuleId, pricing::TaxRuleStatus};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateTaxRuleParams {
    /// URL-safe code, unique within the Store.
    pub code: String,
    pub name: String,
    /// Two-letter ISO 3166-1 country code (e.g. US).
    pub country_code: String,
    /// Tax rate in basis points (100 = 1%).
    pub rate_basis_points: u32,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeTaxRuleStatusParams {
    /// The tax rule's UUID.
    pub tax_rule_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = tax_rules_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List tax rules in the selected Store, including active \
                        and archived ones."
    )]
    async fn list_tax_rules(
        &self,
        Extension(parts): Extension<http::request::Parts>,
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
        let store_id = actor.store_id();

        match self.state.tax_management.list(actor, store_id).await {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(tax_rule_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Create a tax rule in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn create_tax_rule(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateTaxRuleParams>,
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
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .tax_management
            .create(CreateTaxRuleInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                country_code: params.country_code,
                rate_basis_points: params.rate_basis_points,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(tax_rule_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Activate a tax rule in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn activate_tax_rule(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeTaxRuleStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_tax_rule_status(parts, params, TaxRuleStatus::Active)
            .await
    }

    #[tool(description = "Archive a tax rule in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn archive_tax_rule(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeTaxRuleStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_tax_rule_status(parts, params, TaxRuleStatus::Archived)
            .await
    }
}

impl ChaosMcp {
    async fn change_tax_rule_status(
        &self,
        parts: http::request::Parts,
        params: ChangeTaxRuleStatusParams,
        status: TaxRuleStatus,
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
        let store_id = actor.store_id();
        let rule_id = match parse_uuid_field(&params.tax_rule_id, "tax_rule_id") {
            Ok(id) => TaxRuleId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .tax_management
            .change_status(ChangeTaxRuleStatusInput {
                actor,
                store_id,
                rule_id,
                status,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(tax_rule_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn tax_rule_json(detail: TaxRuleDetail) -> serde_json::Value {
    let rule = &detail.rule;
    json!({
        "id": rule.id().as_uuid(),
        "code": rule.code(),
        "name": rule.name(),
        "country_code": rule.country_code(),
        "rate_basis_points": rule.rate_basis_points(),
        "status": rule.status().as_str(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn parse_uuid_field(value: &str, field: &'static str) -> Result<uuid::Uuid, CallToolResult> {
    uuid::Uuid::parse_str(value).map_err(|_| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": format!("{field} must be a valid UUID"),
        }))
    })
}

fn format_time(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
