use chaos_core::contracts::AnalyticsDestination;
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
    tools::{ChaosMcp, StoreIdParams},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureMetaDestinationParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// Meta Dataset ID that receives Conversions API events.
    pub dataset_id: String,
    /// Opaque `enc://...` reference returned by create_provider_secret with kind
    /// `analytics_credential`, or an `env://CHAOS_ANALYTICS_SECRET_*` reference.
    /// The Meta access token itself must never be sent here.
    pub credential_secret_reference: String,
    /// Optional Meta Test Events Code. When present, events are routed to Meta's test view.
    pub test_event_code: Option<String>,
    /// Destination-level switch. Enabling is forward-only; retained historical events are not replayed automatically.
    pub enabled: bool,
    pub confirm: bool,
}

#[tool_router(router = analytics_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Get the Meta Conversions API destination for the selected Store. The destination `enabled` switch controls whether stored behavior events are sent. Credentials are never returned."
    )]
    async fn get_meta_destination(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
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
        description = "Configure the Meta Dataset destination, access-token secret reference, optional Test Events Code, and the forward-only delivery `enabled` switch for the selected Store. Enabling does not replay retained historical events. First call create_provider_secret with kind `analytics_credential` and pass its returned `enc://...` reference here; never pass the raw Meta access token. Owner role and confirmation are required."
    )]
    async fn configure_meta_destination(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureMetaDestinationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts, &params.store_id).await {
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
        let configuration = chaos_core::contracts::AnalyticsDestinationConfiguration {
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
