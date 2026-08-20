use chaos_application::payments::CreateRefundInput;
use chaos_domain::payments::PaymentAttemptId;
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

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateRefundParams {
    /// The payment attempt's UUID.
    pub payment_attempt_id: String,
    /// The refund amount in the payment's smallest currency unit (e.g. cents for USD).
    pub amount_minor: i64,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = payments_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "Refund some or all of a payment attempt in the selected Store. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn create_refund(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateRefundParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
            &self.state.access_key_authentication,
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
        let payment_attempt_id =
            match parse_uuid_field(&params.payment_attempt_id, "payment_attempt_id") {
                Ok(id) => PaymentAttemptId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .payment_service
            .create_refund(CreateRefundInput {
                actor,
                store_id,
                payment_attempt_id,
                amount_minor: params.amount_minor,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(json!({
                "id": detail.id.as_uuid(),
                "payment_attempt_id": detail.payment_attempt_id.as_uuid(),
                "amount_minor": detail.amount_minor,
                "currency": detail.currency.as_str(),
                "status": detail.status.as_str(),
                "provider_reference": detail.provider_reference,
                "failure_code": detail.failure_code,
                "created_at": format_time(detail.created_at),
                "updated_at": format_time(detail.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
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
