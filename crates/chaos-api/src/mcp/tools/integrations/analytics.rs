use chaos_application::ports::{AnalyticsDestination, AnalyticsEventPage, AnalyticsEventQuery};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
    tools::ChaosMcp,
};

#[derive(Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureMetaDestinationParams {
    /// Meta Dataset ID that receives Conversions API events.
    pub dataset_id: String,
    /// Opaque `enc://...` reference returned by create_provider_secret with kind
    /// `analytics_credential`, or an `env://CHAOS_ANALYTICS_SECRET_*` reference.
    /// The Meta access token itself must never be sent here.
    pub credential_secret_reference: String,
    /// Optional Meta Test Events Code. When present, events are routed to Meta's test view.
    pub test_event_code: Option<String>,
    /// Destination-level switch. Enabled destinations receive all subsequently scheduled behavior events.
    pub enabled: bool,
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListAnalyticsEventsParams {
    /// Maximum number of events to return, from 1 to 100. Defaults to 20.
    pub limit: Option<u16>,
    /// Storage row ID returned by a previous page. Only older events are returned.
    pub before_id: Option<String>,
    /// Optional event name filter, such as `page_view`, `purchase`, or `refund`.
    pub event_name: Option<String>,
    /// Optional source filter stored in `properties._source`.
    pub source: Option<String>,
    /// Optional external provider delivery status filter.
    pub delivery_status: Option<AnalyticsDeliveryStatusParam>,
    /// Optional signed shopper identifier for tracing one consumer journey.
    pub shopper_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsDeliveryStatusParam {
    Pending,
    Processed,
    DeadLetter,
}

#[tool_router(router = analytics_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Get the Meta Conversions API destination for the selected Store. The destination `enabled` switch controls whether stored behavior events are sent. Credentials are never returned."
    )]
    async fn get_meta_destination(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let destination = match self
            .state
            .analytics_administration
            .get_destination(actor, store_id, "meta")
            .await
        {
            Ok(destination) => destination,
            Err(error) => return Ok(tool_error(error)),
        };
        Ok(text_result(destination.map_or(Value::Null, meta_json)))
    }

    #[tool(
        description = "Configure the Meta Dataset destination, access-token secret reference, optional Test Events Code, and the delivery `enabled` switch for the selected Store. First call create_provider_secret with kind `analytics_credential` and pass its returned `enc://...` reference here; never pass the raw Meta access token. Owner role and confirmation are required."
    )]
    async fn configure_meta_destination(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureMetaDestinationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        if !(5..=32).contains(&params.dataset_id.len())
            || !params
                .dataset_id
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return Ok(invalid(
                "dataset_id",
                "must contain between 5 and 32 ASCII digits",
            ));
        }
        if params
            .test_event_code
            .as_deref()
            .is_some_and(|code| code.is_empty() || code.len() > 64)
        {
            return Ok(invalid(
                "test_event_code",
                "must contain between 1 and 64 bytes when provided",
            ));
        }
        if !is_analytics_secret_reference(&params.credential_secret_reference) {
            return Ok(invalid(
                "credential_secret_reference",
                "must be an enc:// reference returned by create_provider_secret or an env://CHAOS_ANALYTICS_SECRET_* reference; do not pass the raw Meta access token",
            ));
        }
        let store_id = actor.store_id();
        let configuration = chaos_application::ports::AnalyticsDestinationConfiguration {
            provider: "meta".into(),
            external_account_reference: params.dataset_id,
            credential_secret_reference: params.credential_secret_reference,
            configuration: json!({ "test_event_code": params.test_event_code }),
            enabled: params.enabled,
        };
        let destination = match self
            .state
            .analytics_administration
            .configure_destination(actor, store_id, configuration, self.state.clock.now())
            .await
        {
            Ok(destination) => destination,
            Err(error) => return Ok(tool_error(error)),
        };
        Ok(text_result(meta_json(destination)))
    }

    #[tool(
        description = "List behavior events stored in the selected Store and their external delivery observations. Optional filters include any event name, properties._source, delivery status, and shopper_id. Raw dynamic properties are returned because this tool is intended for internal behavior analysis."
    )]
    async fn list_analytics_events(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListAnalyticsEventsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let limit = params.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Ok(invalid("limit", "must be between 1 and 100"));
        }
        let before_id = match params.before_id.as_deref().map(Uuid::parse_str).transpose() {
            Ok(id) => id,
            Err(_) => return Ok(invalid("before_id", "must be a UUID")),
        };
        let source = params.source;
        let delivery_status = params.delivery_status.map(|status| match status {
            AnalyticsDeliveryStatusParam::Pending => "pending".to_owned(),
            AnalyticsDeliveryStatusParam::Processed => "processed".to_owned(),
            AnalyticsDeliveryStatusParam::DeadLetter => "dead_letter".to_owned(),
        });
        let shopper_id = match params
            .shopper_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
        {
            Ok(id) => id,
            Err(_) => return Ok(invalid("shopper_id", "must be a UUID")),
        };
        let store_id = actor.store_id();
        let query = AnalyticsEventQuery {
            before_id,
            event_name: params.event_name,
            source,
            delivery_status,
            shopper_id,
        };
        match self
            .state
            .analytics_administration
            .list_events(actor, store_id, query, limit)
            .await
        {
            Ok(page) => Ok(text_result(analytics_events_json(page, limit))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn meta_json(item: AnalyticsDestination) -> Value {
    let test_event_code_configured = item
        .configuration
        .get("test_event_code")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    json!({
        "store_id": item.store_id.as_uuid(), "dataset_id": item.external_account_reference,
        "enabled": item.enabled,
        "delivery_enabled": item.enabled,
        "credentials_configured": item.credentials_configured,
        "test_event_code_configured": test_event_code_configured,
        "created_at": item.created_at.to_string(), "updated_at": item.updated_at.to_string(),
    })
}

fn analytics_events_json(page: AnalyticsEventPage, limit: u16) -> Value {
    let next_before_id = page
        .events
        .last()
        .map(|event| event.id)
        .filter(|_| page.has_more);
    json!({
        "events": page.events.into_iter().map(|event| json!({
            "id": event.id,
            "event_id": event.event_id,
            "event_name": event.event_name,
            "shopper_id": event.shopper_id,
            "occurred_at": event.occurred_at.to_string(),
            "received_at": event.received_at.to_string(),
            "properties": event.properties,
            "deliveries": event.deliveries.into_iter().map(|delivery| json!({
                "provider": delivery.provider,
                "status": delivery.status,
                "delivered_at": delivery.delivered_at.map(|value| value.to_string()),
                "provider_reference": delivery.provider_reference,
                "last_error": delivery.last_error,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_before_id": next_before_id,
        "limit": limit,
    })
}

fn invalid(field: &'static str, message: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params", "message": format!("{field} {message}"),
    }))
}

fn is_analytics_secret_reference(value: &str) -> bool {
    if let Some(encoded) = value.strip_prefix("enc://") {
        return value.len() <= 518
            && !encoded.is_empty()
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    }
    let Some(variable) = value.strip_prefix("env://") else {
        return false;
    };
    let prefix = "CHAOS_ANALYTICS_SECRET_";
    let suffix = variable.strip_prefix(prefix).unwrap_or_default();
    !suffix.is_empty()
        && suffix.len() <= 96
        && variable
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::is_analytics_secret_reference;

    #[test]
    fn accepts_only_supported_analytics_secret_references() {
        assert!(is_analytics_secret_reference("enc://encrypted-reference_1"));
        assert!(is_analytics_secret_reference(
            "env://CHAOS_ANALYTICS_SECRET_META"
        ));
        assert!(!is_analytics_secret_reference("EAABraw-meta-token"));
        assert!(!is_analytics_secret_reference(
            "env://CHAOS_PAYMENT_SECRET_META"
        ));
        assert!(!is_analytics_secret_reference("enc://"));
    }
}
