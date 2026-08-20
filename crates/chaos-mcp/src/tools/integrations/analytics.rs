use chaos_application::{
    analytics::{RequestAnalyticsErasureInput, UpdateAnalyticsPolicyInput},
    ports::{
        AnalyticsDailyReports, AnalyticsDestinationAccount, AnalyticsDestinationConfiguration,
        AnalyticsErasureRequest, AnalyticsErasureSelector, StoreAnalyticsPolicy,
    },
};
use chaos_domain::{
    analytics::{AnalyticsDestinationProvider, AnalyticsDestinationSecretReference},
    sales::CustomerId,
};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateAnalyticsPolicyParams {
    pub behavior_collection_enabled: bool,
    pub advertising_exports_enabled: bool,
    pub identity_linking_enabled: bool,
    pub raw_event_retention_days: u16,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RequestAnalyticsErasureParams {
    #[serde(default)]
    pub anonymous_id: Option<String>,
    #[serde(default)]
    pub customer_id: Option<String>,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetAnalyticsErasureParams {
    pub request_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListDailyAnalyticsReportsParams {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureAnalyticsDestinationParams {
    pub provider: String,
    pub external_destination_reference: String,
    #[serde(default)]
    pub event_source_base_url: Option<String>,
    pub credential_secret_reference: String,
    pub enabled: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[tool_router(router = analytics_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(description = "Get the analytics policy for the selected Store.")]
    async fn get_analytics_policy(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .analytics_administration
            .get_policy(actor, store_id, self.state.clock.now())
            .await
        {
            Ok(policy) => Ok(text_result(policy_json(policy))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Update the analytics policy for the selected Store.")]
    async fn update_analytics_policy(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateAnalyticsPolicyParams>,
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
        let input = UpdateAnalyticsPolicyInput {
            actor,
            store_id,
            behavior_collection_enabled: params.behavior_collection_enabled,
            advertising_exports_enabled: params.advertising_exports_enabled,
            identity_linking_enabled: params.identity_linking_enabled,
            raw_event_retention_days: params.raw_event_retention_days,
            idempotency,
            now: self.state.clock.now(),
        };
        match self
            .state
            .analytics_administration
            .update_policy(input)
            .await
        {
            Ok(policy) => Ok(text_result(policy_json(policy))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Request analytics data erasure for the selected Store.")]
    async fn request_analytics_erasure(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RequestAnalyticsErasureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let selector = match (&params.anonymous_id, &params.customer_id) {
            (Some(value), None) => match uuid::Uuid::parse_str(value) {
                Ok(id) => AnalyticsErasureSelector::Anonymous(id),
                Err(_) => return Ok(invalid("anonymous_id", "must be a valid UUID")),
            },
            (None, Some(value)) => match uuid::Uuid::parse_str(value) {
                Ok(id) => AnalyticsErasureSelector::Customer(CustomerId::from_uuid(id)),
                Err(_) => return Ok(invalid("customer_id", "must be a valid UUID")),
            },
            _ => {
                return Ok(invalid(
                    "selector",
                    "provide exactly one analytics identifier",
                ));
            }
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .analytics_privacy
            .request_erasure(RequestAnalyticsErasureInput {
                actor,
                store_id,
                selector,
                idempotency,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(request) => Ok(text_result(erasure_json(request))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Get an analytics erasure request in the selected Store.")]
    async fn get_analytics_erasure_request(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetAnalyticsErasureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let request_id = match uuid::Uuid::parse_str(&params.request_id) {
            Ok(id) => id,
            Err(_) => return Ok(invalid("request_id", "must be a valid UUID")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .analytics_privacy
            .get_erasure_request(actor, store_id, request_id)
            .await
        {
            Ok(request) => Ok(text_result(erasure_json(request))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "List daily analytics reports for the selected Store.")]
    async fn list_daily_analytics_reports(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListDailyAnalyticsReportsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let from = match parse_date(&params.from) {
            Some(value) => value,
            None => return Ok(invalid("from", "must be an ISO 8601 calendar date")),
        };
        let to = match parse_date(&params.to) {
            Some(value) => value,
            None => return Ok(invalid("to", "must be an ISO 8601 calendar date")),
        };
        let store_id = actor.store_id();
        match self
            .state
            .analytics_reporting
            .list_daily_reports(actor, store_id, from, to)
            .await
        {
            Ok(reports) => Ok(text_result(reports_json(reports))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "List analytics export destinations for the selected Store.")]
    async fn list_analytics_destinations(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .analytics_destinations
            .list(actor, store_id)
            .await
        {
            Ok(items) => Ok(text_result(Value::Array(
                items.into_iter().map(destination_json).collect(),
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create or update an analytics export destination for the selected Store."
    )]
    async fn configure_analytics_destination(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureAnalyticsDestinationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let provider = match AnalyticsDestinationProvider::parse(&params.provider) {
            Some(provider) => provider,
            None => return Ok(invalid("provider", "must be meta_capi or ga4")),
        };
        let secret = match AnalyticsDestinationSecretReference::new(
            params.credential_secret_reference.clone(),
        ) {
            Ok(secret) => secret,
            Err(_) => return Ok(invalid("credential_secret_reference", "is invalid")),
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let configuration = AnalyticsDestinationConfiguration {
            provider,
            external_destination_reference: params.external_destination_reference,
            event_source_base_url: params.event_source_base_url,
            credential_secret_reference: secret,
            enabled: params.enabled,
        };
        match self
            .state
            .analytics_destinations
            .configure(
                actor,
                store_id,
                configuration,
                idempotency,
                self.state.clock.now(),
            )
            .await
        {
            Ok(item) => Ok(text_result(destination_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_date(value: &str) -> Option<time::Date> {
    let format = time::format_description::parse_borrowed::<3>("[year]-[month]-[day]").ok()?;
    time::Date::parse(value, &format).ok()
}

fn policy_json(item: StoreAnalyticsPolicy) -> Value {
    json!({
        "store_id": item.store_id.as_uuid(),
        "policy_version": item.policy_version,
        "behavior_collection_enabled": item.policy.behavior_collection_enabled(),
        "advertising_exports_enabled": item.policy.advertising_exports_enabled(),
        "identity_linking_enabled": item.policy.identity_linking_enabled(),
        "raw_event_retention_days": item.policy.raw_event_retention_days(),
        "created_by": item.created_by.map(|id| id.as_uuid()),
        "effective_at": item.effective_at.map(|value| value.to_string()),
        "created_at": item.created_at.map(|value| value.to_string()),
    })
}

fn erasure_json(item: AnalyticsErasureRequest) -> Value {
    let (selector_kind, selector_id) = match item.selector {
        AnalyticsErasureSelector::Anonymous(id) => ("anonymous", id),
        AnalyticsErasureSelector::Customer(id) => ("customer", id.as_uuid()),
    };
    json!({
        "id": item.id,
        "store_id": item.store_id.as_uuid(),
        "selector_kind": selector_kind,
        "selector_id": selector_id,
        "status": format!("{:?}", item.status).to_lowercase(),
        "requested_by": item.requested_by.as_uuid(),
        "behavior_events_deleted": item.behavior_events_deleted,
        "attribution_results_deleted": item.attribution_results_deleted,
        "sessions_deleted": item.sessions_deleted,
        "identity_links_deleted": item.identity_links_deleted,
        "requested_at": item.requested_at.to_string(),
        "completed_at": item.completed_at.map(|value| value.to_string()),
    })
}

fn destination_json(item: AnalyticsDestinationAccount) -> Value {
    json!({
        "id": item.id,
        "store_id": item.store_id.as_uuid(),
        "provider": item.provider.as_str(),
        "external_destination_reference": item.external_destination_reference,
        "event_source_base_url": item.event_source_base_url,
        "enabled": item.enabled,
        "credentials_configured": item.credentials_configured,
        "created_at": item.created_at.to_string(),
        "updated_at": item.updated_at.to_string(),
    })
}

fn reports_json(reports: AnalyticsDailyReports) -> Value {
    json!({
        "behavior": reports.behavior.into_iter().map(|item| json!({
            "sales_channel_id": item.sales_channel_id.as_uuid(), "report_date": item.report_date.to_string(),
            "sessions": item.sessions, "events": item.events, "page_views": item.page_views,
            "product_views": item.product_views, "searches": item.searches,
            "cart_line_additions": item.cart_line_additions, "checkouts_started": item.checkouts_started,
            "active_engagement_milliseconds": item.active_engagement_milliseconds,
            "refreshed_at": item.refreshed_at.to_string(),
        })).collect::<Vec<_>>(),
        "commerce": reports.commerce.into_iter().map(|item| json!({
            "sales_channel_id": item.sales_channel_id.as_uuid(), "report_date": item.report_date.to_string(),
            "currency": item.currency, "orders_created": item.orders_created,
            "order_amount_minor": item.order_amount_minor, "payments_captured": item.payments_captured,
            "captured_amount_minor": item.captured_amount_minor, "refunds_succeeded": item.refunds_succeeded,
            "refunded_amount_minor": item.refunded_amount_minor, "fulfillments_shipped": item.fulfillments_shipped,
            "returns_completed": item.returns_completed, "refreshed_at": item.refreshed_at.to_string(),
        })).collect::<Vec<_>>(),
        "attribution": reports.attribution.into_iter().map(|item| json!({
            "sales_channel_id": item.sales_channel_id.as_uuid(), "report_date": item.report_date.to_string(),
            "attribution_model": item.attribution_model, "model_version": item.model_version,
            "is_direct": item.is_direct, "campaign_source": item.campaign_source,
            "campaign_medium": item.campaign_medium, "campaign_name": item.campaign_name,
            "attributed_orders": item.attributed_orders, "attributed_amount_minor": item.attributed_amount_minor,
            "currency": item.currency, "refreshed_at": item.refreshed_at.to_string(),
        })).collect::<Vec<_>>(),
    })
}

fn invalid(field: &'static str, message: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field} {message}"),
    }))
}
