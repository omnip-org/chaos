use chaos_application::{
    ports::PromotionDetail,
    pricing::{ChangePromotionStatusInput, CreatePromotionInput},
};
use chaos_domain::{
    CurrencyCode,
    pricing::{PromotionId, PromotionStatus, PromotionTrigger, PromotionValue},
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
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreatePromotionParams {
    /// URL-safe code, unique within the Store.
    pub handle: String,
    pub name: String,
    /// "automatic" (applies to every eligible order) or "code" (requires redemption_code).
    pub trigger: String,
    /// Required when trigger is "code"; must be unique among active promotions in the Store.
    #[serde(default)]
    pub redemption_code: Option<String>,
    /// "percentage" or "fixed_amount".
    pub value_kind: String,
    /// Required when value_kind is "percentage". 1-10000 (100 = 1%).
    #[serde(default)]
    pub rate_basis_points: Option<u32>,
    /// Required when value_kind is "fixed_amount", in the promotion's smallest currency unit.
    #[serde(default)]
    pub amount_minor: Option<i64>,
    /// Optional cap on the discount amount for a percentage promotion, in minor units.
    #[serde(default)]
    pub maximum_amount_minor: Option<i64>,
    /// Three-letter ISO 4217 currency code (e.g. USD).
    pub currency: String,
    #[serde(default)]
    pub minimum_subtotal_amount_minor: i64,
    /// Lower values apply first when multiple promotions are eligible.
    #[serde(default)]
    pub priority: u16,
    /// RFC 3339 timestamp; omit for no start boundary.
    #[serde(default)]
    pub starts_at: Option<String>,
    /// RFC 3339 timestamp; omit for no end boundary.
    #[serde(default)]
    pub ends_at: Option<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangePromotionStatusParams {
    /// The promotion's UUID.
    pub promotion_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = promotions_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "List promotions in the selected Store, including active \
                        and archived ones."
    )]
    async fn list_promotions(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
            &self.state.mcp_key_authentication,
            &self.state.merchant_queries,
            &parts,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();

        match self.state.promotion_management.list(actor, store_id).await {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(promotion_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Create a promotion in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn create_promotion(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreatePromotionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
            &self.state.mcp_key_authentication,
            &self.state.merchant_queries,
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
        let trigger = match PromotionTrigger::parse(&params.trigger) {
            Some(trigger) => trigger,
            None => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "trigger must be \"automatic\" or \"code\"",
                })));
            }
        };
        let value = match promotion_value(&params) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let currency = match CurrencyCode::parse(&params.currency) {
            Ok(currency) => currency,
            Err(_) => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "currency must be a valid ISO 4217 code",
                })));
            }
        };
        let starts_at = match parse_optional_time("starts_at", params.starts_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let ends_at = match parse_optional_time("ends_at", params.ends_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .promotion_management
            .create(CreatePromotionInput {
                actor,
                store_id,
                handle: params.handle,
                name: params.name,
                trigger,
                redemption_code: params.redemption_code,
                value,
                currency,
                minimum_subtotal_amount_minor: params.minimum_subtotal_amount_minor,
                priority: params.priority,
                starts_at,
                ends_at,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(promotion_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Activate a promotion in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn activate_promotion(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangePromotionStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_promotion_status(parts, params, PromotionStatus::Active)
            .await
    }

    #[tool(description = "Archive a promotion in the selected Store. Requires \
                        confirm: true and an idempotency_key.")]
    async fn archive_promotion(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangePromotionStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_promotion_status(parts, params, PromotionStatus::Archived)
            .await
    }
}

impl ChaosMcp {
    async fn change_promotion_status(
        &self,
        parts: http::request::Parts,
        params: ChangePromotionStatusParams,
        status: PromotionStatus,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
            &self.state.mcp_key_authentication,
            &self.state.merchant_queries,
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
        let promotion_id = match parse_uuid_field(&params.promotion_id, "promotion_id") {
            Ok(id) => PromotionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .promotion_management
            .change_status(ChangePromotionStatusInput {
                actor,
                store_id,
                promotion_id,
                status,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(promotion_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn promotion_value(params: &CreatePromotionParams) -> Result<PromotionValue, CallToolResult> {
    match params.value_kind.as_str() {
        "percentage" => {
            let rate_basis_points = params.rate_basis_points.ok_or_else(|| {
                CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "rate_basis_points is required when value_kind is \"percentage\"",
                }))
            })?;
            Ok(PromotionValue::Percentage {
                rate_basis_points,
                maximum_amount_minor: params.maximum_amount_minor,
            })
        }
        "fixed_amount" => {
            let amount_minor = params.amount_minor.ok_or_else(|| {
                CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "amount_minor is required when value_kind is \"fixed_amount\"",
                }))
            })?;
            Ok(PromotionValue::FixedAmount { amount_minor })
        }
        _ => Err(CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": "value_kind must be \"percentage\" or \"fixed_amount\"",
        }))),
    }
}

fn promotion_json(detail: PromotionDetail) -> serde_json::Value {
    let promotion = &detail.promotion;
    json!({
        "id": promotion.id().as_uuid(),
        "handle": promotion.handle(),
        "name": promotion.name(),
        "trigger": promotion.trigger().as_str(),
        "redemption_code": promotion.redemption_code(),
        "value_kind": promotion.value().kind(),
        "rate_basis_points": promotion.value().rate_basis_points(),
        "amount_minor": promotion.value().amount_minor(),
        "maximum_amount_minor": promotion.value().maximum_amount_minor(),
        "currency": promotion.currency().as_str(),
        "minimum_subtotal_amount_minor": promotion.minimum_subtotal_amount_minor(),
        "priority": promotion.priority(),
        "starts_at": promotion.starts_at().map(format_time),
        "ends_at": promotion.ends_at().map(format_time),
        "status": promotion.status().as_str(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn parse_optional_time(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<OffsetDateTime>, CallToolResult> {
    value
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
                CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": format!("{field} must be an RFC 3339 timestamp"),
                }))
            })
        })
        .transpose()
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
