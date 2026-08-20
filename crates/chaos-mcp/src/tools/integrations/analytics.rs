use chaos_application::{
    analytics::{RequestAnalyticsErasureInput, UpdateAnalyticsSettingsInput},
    ports::{
        AnalyticsErasureRequest, AnalyticsErasureSelector, MetaConnection,
        MetaConnectionConfiguration, StoreAnalyticsSettings,
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
    pub collection_enabled: bool,
    pub browser_collection_mode: BrowserCollectionModeParam,
    pub meta_reporting_enabled: bool,
    pub identity_linking_enabled: bool,
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
    pub dataset_id: String,
    pub credential_secret_reference: String,
    pub test_event_code: Option<String>,
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

#[tool_router(router = analytics_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(description = "Get Analytics settings for the selected Store.")]
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
        description = "Update the opt-in or opt-out browser collection mode, Analytics collection, Meta reporting, identity linking, and retention settings for the selected Store. Owner role and confirmation are required."
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

    #[tool(description = "Get the Meta Conversions API connection for the selected Store.")]
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
        match self
            .state
            .analytics_administration
            .get_meta_connection(actor, store_id)
            .await
        {
            Ok(connection) => Ok(text_result(connection.map_or(Value::Null, meta_json))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Configure the Meta Conversions API connection for the selected Store. Owner role and confirmation are required."
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
        match self
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
            Ok(connection) => Ok(text_result(meta_json(connection))),
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

fn meta_json(item: MetaConnection) -> Value {
    json!({
        "store_id": item.store_id.as_uuid(), "dataset_id": item.dataset_id,
        "capi_enabled": item.capi_enabled,
        "credentials_configured": item.credentials_configured,
        "test_event_code_configured": item.test_event_code_configured,
        "created_at": item.created_at.to_string(), "updated_at": item.updated_at.to_string(),
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
