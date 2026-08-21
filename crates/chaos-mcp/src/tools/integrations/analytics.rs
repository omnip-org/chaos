use chaos_application::{
    analytics::{RequestAnalyticsErasureInput, UpdateAnalyticsSettingsInput},
    ports::{
        AnalyticsErasureRequest, AnalyticsErasureSelector, AnalyticsEventPage, AnalyticsEventQuery,
        MetaConnection, MetaConnectionConfiguration, StoreAnalyticsSettings,
    },
};
use chaos_domain::{analytics::BrowserCollectionMode, sales::CustomerId};
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

use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
    tools::ChaosMcp,
};

#[derive(Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateAnalyticsSettingsParams {
    /// Master switch for storing Analytics events in this Store.
    pub collection_enabled: bool,
    /// Consent policy used by browser collection. `opt_out` permits collection under the Store policy; `opt_in` requires consent.
    pub browser_collection_mode: BrowserCollectionModeParam,
    /// Store-level switch for creating Meta delivery work. This must be true together with a configured connection whose `capi_enabled` is true.
    pub meta_reporting_enabled: bool,
    /// Whether visitor-to-customer identity linking is enabled.
    pub identity_linking_enabled: bool,
    /// Number of days to retain the internal event ledger.
    pub raw_event_retention_days: u16,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserCollectionModeParam {
    OptIn,
    OptOut,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigureMetaConnectionParams {
    /// Meta Dataset ID that receives Conversions API events.
    pub dataset_id: String,
    /// Secret reference for the Meta access token. The secret value itself must never be sent here.
    pub credential_secret_reference: String,
    /// Optional Meta Test Events Code. When present, events are routed to Meta's test view.
    pub test_event_code: Option<String>,
    /// Connection-level switch. This only enables this Meta connection; Store `meta_reporting_enabled` must also be true before events are queued.
    pub capi_enabled: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErasureSelectorKind {
    Visitor,
    Customer,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RequestAnalyticsErasureParams {
    pub selector_kind: ErasureSelectorKind,
    pub selector_id: String,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetAnalyticsErasureParams {
    pub request_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListAnalyticsEventsParams {
    /// Maximum number of events to return, from 1 to 100. Defaults to 20.
    pub limit: Option<u16>,
    /// Storage row ID returned by a previous page. Only older events are returned.
    pub before_id: Option<String>,
    /// Optional event name filter, such as `page_view`, `purchase`, or `refund`.
    pub event_name: Option<String>,
    /// Optional event source filter.
    pub source: Option<AnalyticsEventSourceParam>,
    /// Optional Meta delivery status filter.
    pub delivery_status: Option<MetaDeliveryStatusParam>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsEventSourceParam {
    Browser,
    Server,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetaDeliveryStatusParam {
    Pending,
    Processed,
    DeadLetter,
}

#[tool_router(router = analytics_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Get Analytics settings for the selected Store. `meta_reporting_enabled` is the Store-level switch that allows events to be queued for Meta; it is separate from the Meta connection's `capi_enabled` switch."
    )]
    async fn get_analytics_settings(
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
            .get_settings(actor, store_id, self.state.clock.now())
            .await
        {
            Ok(settings) => Ok(text_result(settings_json(settings))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update the Analytics Store policy. `meta_reporting_enabled` controls whether eligible events are queued for Meta. It does not configure Meta credentials or Dataset delivery; Meta delivery requires both this switch and a Meta connection with `capi_enabled: true`. Owner role and confirmation are required."
    )]
    async fn update_analytics_settings(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateAnalyticsSettingsParams>,
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
            .analytics_administration
            .update_settings(UpdateAnalyticsSettingsInput {
                actor,
                store_id,
                collection_enabled: params.collection_enabled,
                browser_collection_mode: match params.browser_collection_mode {
                    BrowserCollectionModeParam::OptIn => BrowserCollectionMode::OptIn,
                    BrowserCollectionModeParam::OptOut => BrowserCollectionMode::OptOut,
                },
                meta_reporting_enabled: params.meta_reporting_enabled,
                identity_linking_enabled: params.identity_linking_enabled,
                raw_event_retention_days: params.raw_event_retention_days,
                idempotency,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(settings) => Ok(text_result(settings_json(settings))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get the Meta Conversions API connection for the selected Store. `capi_enabled` is the connection-level delivery switch; Meta events are sent only when it and the Store-level `meta_reporting_enabled` switch are both true. Credentials are never returned."
    )]
    async fn get_meta_connection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let connection = match self
            .state
            .analytics_administration
            .get_meta_connection(actor, store_id)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return Ok(tool_error(error)),
        };
        let settings = match self
            .state
            .analytics_administration
            .get_settings(actor, store_id, self.state.clock.now())
            .await
        {
            Ok(settings) => settings,
            Err(error) => return Ok(tool_error(error)),
        };
        Ok(text_result(connection.map_or(Value::Null, |item| {
            meta_json(item, settings.settings.meta_reporting_enabled())
        })))
    }

    #[tool(
        description = "Configure the Meta Dataset, access-token secret reference, optional Test Events Code, and connection-level `capi_enabled` switch for the selected Store. This does not turn on Store-level Meta reporting; use `update_analytics_settings` with `meta_reporting_enabled: true` as well. Owner role and confirmation are required."
    )]
    async fn configure_meta_connection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ConfigureMetaConnectionParams>,
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
        let configuration = MetaConnectionConfiguration {
            dataset_id: params.dataset_id,
            credential_secret_reference: params.credential_secret_reference,
            test_event_code: params.test_event_code,
            capi_enabled: params.capi_enabled,
        };
        let connection = match self
            .state
            .analytics_administration
            .configure_meta_connection(
                actor,
                store_id,
                configuration,
                &idempotency,
                self.state.clock.now(),
            )
            .await
        {
            Ok(connection) => connection,
            Err(error) => return Ok(tool_error(error)),
        };
        let settings = match self
            .state
            .analytics_administration
            .get_settings(actor, store_id, self.state.clock.now())
            .await
        {
            Ok(settings) => settings,
            Err(error) => return Ok(tool_error(error)),
        };
        Ok(text_result(meta_json(
            connection,
            settings.settings.meta_reporting_enabled(),
        )))
    }

    #[tool(
        description = "List events stored in the selected Store's internal Analytics ledger and the corresponding Meta delivery observation. Use this to distinguish events that were not eligible, were queued as pending, were processed by Meta, or reached dead-letter status. Store members can read event metadata and provider errors; raw event properties are not returned."
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
        let source = params.source.map(|source| match source {
            AnalyticsEventSourceParam::Browser => "browser".to_owned(),
            AnalyticsEventSourceParam::Server => "server".to_owned(),
        });
        let delivery_status = params.delivery_status.map(|status| match status {
            MetaDeliveryStatusParam::Pending => "pending".to_owned(),
            MetaDeliveryStatusParam::Processed => "processed".to_owned(),
            MetaDeliveryStatusParam::DeadLetter => "dead_letter".to_owned(),
        });
        let store_id = actor.store_id();
        let query = AnalyticsEventQuery {
            before_id,
            event_name: params.event_name,
            source,
            delivery_status,
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

    #[tool(
        description = "Request deletion of Analytics data for a Visitor or Customer in the selected Store. Owner role and confirmation are required."
    )]
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
        let id = match Uuid::parse_str(&params.selector_id) {
            Ok(id) => id,
            Err(_) => return Ok(invalid("selector_id", "must be a UUID")),
        };
        let selector = match params.selector_kind {
            ErasureSelectorKind::Visitor => AnalyticsErasureSelector::Visitor(id),
            ErasureSelectorKind::Customer => {
                AnalyticsErasureSelector::Customer(CustomerId::from_uuid(id))
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

    #[tool(description = "Get an Analytics erasure request in the selected Store.")]
    async fn get_analytics_erasure_request(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetAnalyticsErasureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let request_id = match Uuid::parse_str(&params.request_id) {
            Ok(id) => id,
            Err(_) => return Ok(invalid("request_id", "must be a UUID")),
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
}

fn settings_json(item: StoreAnalyticsSettings) -> Value {
    json!({
        "store_id": item.store_id.as_uuid(), "revision": item.revision,
        "collection_enabled": item.settings.collection_enabled(),
        "browser_collection_mode": item.settings.browser_collection_mode().as_str(),
        "meta_reporting_enabled": item.settings.meta_reporting_enabled(),
        "identity_linking_enabled": item.settings.identity_linking_enabled(),
        "raw_event_retention_days": item.settings.raw_event_retention_days(),
        "updated_by": item.updated_by.map(|id| id.as_uuid()),
        "updated_at": item.updated_at.map(|value| value.to_string()),
    })
}

fn meta_json(item: MetaConnection, meta_reporting_enabled: bool) -> Value {
    json!({
        "store_id": item.store_id.as_uuid(), "dataset_id": item.dataset_id,
        "capi_enabled": item.capi_enabled,
        "meta_reporting_enabled": meta_reporting_enabled,
        "meta_delivery_enabled": item.capi_enabled && meta_reporting_enabled,
        "credentials_configured": item.credentials_configured,
        "test_event_code_configured": item.test_event_code_configured,
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
            "source": event.source,
            "occurred_at": event.occurred_at.to_string(),
            "received_at": event.received_at.to_string(),
            "meta_eligible": event.meta_eligible,
            "meta_delivery": event.meta_delivery_status.map(|status| json!({
                "status": status,
                "delivered_at": event.meta_delivered_at.map(|value| value.to_string()),
                "provider_reference": event.meta_provider_reference,
                "last_error": event.meta_last_error,
            })),
        })).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_before_id": next_before_id,
        "limit": limit,
    })
}

fn erasure_json(item: AnalyticsErasureRequest) -> Value {
    let (kind, id) = match item.selector {
        AnalyticsErasureSelector::Visitor(id) => ("visitor", id),
        AnalyticsErasureSelector::Customer(id) => ("customer", id.as_uuid()),
    };
    json!({
        "id": item.id, "store_id": item.store_id.as_uuid(),
        "selector_kind": kind, "selector_id": id,
        "status": format!("{:?}", item.status).to_lowercase(),
        "requested_by": item.requested_by.as_uuid(),
        "commerce_events_deleted": item.commerce_events_deleted,
        "visitor_links_deleted": item.visitor_links_deleted,
        "requested_at": item.requested_at.to_string(),
        "completed_at": item.completed_at.map(|value| value.to_string()),
    })
}

fn invalid(field: &'static str, message: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params", "message": format!("{field} {message}"),
    }))
}
