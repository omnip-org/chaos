use chaos_application::{
    fulfillment::{
        CancelPurchasedShippingLabelInput, ChangeShippingServiceStatusInput,
        CreateFulfillmentInput, CreateReturnInput, CreateShippingProviderAccountInput,
        CreateShippingServiceInput, PurchaseShippingLabelInput, QuoteShippingRatesInput,
        TransitionFulfillmentInput, TransitionReturnInput, UpdateShippingProviderAccountInput,
    },
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, ReturnDetail, ReturnLineInput,
        ReturnReceiptInput, ShippingAddress, ShippingLabelDetail, ShippingParcel,
        ShippingProviderAccountDetail, ShippingRateQuoteDetail, ShippingServiceDetail,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{
        FulfillmentId, FulfillmentStatus, ReturnDisposition, ReturnId, ReturnStatus,
        ShippingProviderAccountId, ShippingRateQuoteId, ShippingServiceId, ShippingServiceStatus,
    },
    inventory::InventoryLocationId,
    sales::OrderId,
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
use time::format_description::well_known::Rfc3339;

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct FulfillmentAllocationParams {
    /// The product variant's UUID.
    pub product_variant_id: String,
    pub quantity: u32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateFulfillmentParams {
    /// The order's UUID.
    pub order_id: String,
    pub allocations: Vec<FulfillmentAllocationParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct TransitionFulfillmentParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// Target status: shipped, delivered, or cancelled.
    pub target_status: String,
    #[serde(default)]
    pub carrier: Option<String>,
    #[serde(default)]
    pub tracking_number: Option<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReturnLineParams {
    /// The product variant's UUID.
    pub product_variant_id: String,
    pub quantity: u32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateReturnParams {
    /// The order's UUID.
    pub order_id: String,
    pub lines: Vec<ReturnLineParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetReturnParams {
    /// The return's UUID.
    pub return_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReturnReceiptParams {
    /// The product variant's UUID.
    pub product_variant_id: String,
    /// "restock" or "discard".
    pub disposition: String,
    /// Required when disposition is "restock".
    #[serde(default)]
    pub inventory_location_id: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct TransitionReturnParams {
    /// The return's UUID.
    pub return_id: String,
    /// Target status: authorized, rejected, received, or completed.
    pub target_status: String,
    /// Required when target_status is "received": what happened to each returned unit.
    #[serde(default)]
    pub receipt: Vec<ReturnReceiptParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateShippingServiceParams {
    /// URL-safe code, unique within the Store.
    pub code: String,
    pub name: String,
    /// Three-letter ISO 4217 currency code (e.g. USD).
    pub currency: String,
    pub amount_minor: i64,
    pub estimated_min_days: u16,
    pub estimated_max_days: u16,
    /// ISO 3166-1 alpha-2 country codes this service ships to.
    pub destination_countries: Vec<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeShippingServiceStatusParams {
    /// The shipping service's UUID.
    pub shipping_service_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetShippingProviderAccountParams {
    /// The shipping provider account's UUID.
    pub provider_account_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ShippingOriginParams {
    pub name: String,
    #[serde(default)]
    pub company: Option<String>,
    pub address_line_1: String,
    #[serde(default)]
    pub address_line_2: Option<String>,
    pub city: String,
    #[serde(default)]
    pub region: Option<String>,
    pub postal_code: String,
    /// ISO 3166-1 alpha-2 country code.
    pub country_code: String,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateShippingProviderAccountParams {
    /// The shipping provider adapter name (deployment-specific, e.g. "fedex", "ups").
    pub provider: String,
    pub display_name: String,
    /// Opaque reference to the provider credentials (deployment-specific secret store).
    pub credential_secret_reference: String,
    pub origin: ShippingOriginParams,
    #[serde(default)]
    pub enabled: bool,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateShippingProviderAccountParams {
    /// The shipping provider account's UUID.
    pub provider_account_id: String,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub origin: ShippingOriginParams,
    pub enabled: bool,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct QuoteShippingRatesParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// The shipping provider account's UUID.
    pub provider_account_id: String,
    pub length_millimetres: u32,
    pub width_millimetres: u32,
    pub height_millimetres: u32,
    pub weight_grams: u32,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PurchaseShippingLabelParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// The rate quote's UUID, from quote_shipping_rates.
    pub rate_quote_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CancelShippingLabelParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = fulfillment_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "Create a fulfillment for an order in the selected Store, \
                        allocating specific variant quantities to ship. Requires confirm: true \
                        and an idempotency_key."
    )]
    async fn create_fulfillment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateFulfillmentParams>,
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
        let order_id = match parse_uuid_field(&params.order_id, "order_id") {
            Ok(id) => OrderId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let allocations = match parse_allocations(&params.allocations) {
            Ok(allocations) => allocations,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .fulfillment_management
            .create_fulfillment(CreateFulfillmentInput {
                actor,
                store_id,
                order_id,
                allocations,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Transition a fulfillment's status (e.g. shipped, delivered, cancelled) \
                        in the selected Store. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn transition_fulfillment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<TransitionFulfillmentParams>,
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
        let fulfillment_id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let target_status = match parse_fulfillment_status(&params.target_status) {
            Ok(status) => status,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .fulfillment_management
            .transition_fulfillment(TransitionFulfillmentInput {
                actor,
                store_id,
                fulfillment_id,
                target_status,
                carrier: params.carrier,
                tracking_number: params.tracking_number,
                now,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Create a return for an order in the selected Store. \
                        Requires confirm: true and an idempotency_key.")]
    async fn create_return(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateReturnParams>,
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
        let order_id = match parse_uuid_field(&params.order_id, "order_id") {
            Ok(id) => OrderId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let lines = match parse_return_lines(&params.lines) {
            Ok(lines) => lines,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .fulfillment_management
            .create_return(CreateReturnInput {
                actor,
                store_id,
                order_id,
                lines,
                now,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(return_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Get a single return's details in the selected Store.")]
    async fn get_return(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetReturnParams>,
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
        let return_id = match parse_uuid_field(&params.return_id, "return_id") {
            Ok(id) => ReturnId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .fulfillment_management
            .get_return(actor, store_id, return_id)
            .await
        {
            Ok(detail) => Ok(text_result(return_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Transition a return's status (authorize, reject, receive, complete) in \
                        the selected Store. When transitioning to \"received\", \
                        provide a receipt line per returned variant describing its disposition. \
                        Requires confirm: true and an idempotency_key."
    )]
    async fn transition_return(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<TransitionReturnParams>,
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
        let return_id = match parse_uuid_field(&params.return_id, "return_id") {
            Ok(id) => ReturnId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let target_status = match parse_return_status(&params.target_status) {
            Ok(status) => status,
            Err(result) => return Ok(result),
        };
        let receipt = match parse_receipt(&params.receipt) {
            Ok(receipt) => receipt,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .fulfillment_management
            .transition_return(TransitionReturnInput {
                actor,
                store_id,
                return_id,
                target_status,
                receipt,
                now,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(return_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List shipping services in the selected Store, including \
                        archived ones."
    )]
    async fn list_shipping_services(
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

        match self.state.shipping_management.list(actor, store_id).await {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(shipping_service_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a shipping service in the selected Store. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn create_shipping_service(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateShippingServiceParams>,
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
            .shipping_management
            .create(CreateShippingServiceInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                currency: params.currency,
                amount_minor: params.amount_minor,
                estimated_min_days: params.estimated_min_days,
                estimated_max_days: params.estimated_max_days,
                destination_countries: params.destination_countries,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(shipping_service_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Activate a shipping service in the selected Store. \
                        Requires confirm: true and an idempotency_key.")]
    async fn activate_shipping_service(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeShippingServiceStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_shipping_service_status(parts, params, ShippingServiceStatus::Active)
            .await
    }

    #[tool(description = "Archive a shipping service in the selected Store. \
                        Requires confirm: true and an idempotency_key.")]
    async fn archive_shipping_service(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeShippingServiceStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_shipping_service_status(parts, params, ShippingServiceStatus::Archived)
            .await
    }

    #[tool(
        description = "List shipping provider accounts (carrier integrations) in the Store \
                        selected by X-Chaos-Store-Id."
    )]
    async fn list_shipping_provider_accounts(
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

        match self
            .state
            .shipping_provider_administration
            .list(actor, store_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(provider_account_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Get a single shipping provider account's details in the selected Store.")]
    async fn get_shipping_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetShippingProviderAccountParams>,
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
        let provider_account_id =
            match parse_uuid_field(&params.provider_account_id, "provider_account_id") {
                Ok(id) => ShippingProviderAccountId::from_uuid(id),
                Err(result) => return Ok(result),
            };

        match self
            .state
            .shipping_provider_administration
            .get(actor, store_id, provider_account_id)
            .await
        {
            Ok(detail) => Ok(text_result(provider_account_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a shipping provider account (carrier integration) in the Store \
                        selected by X-Chaos-Store-Id. Requires confirm: true and an idempotency_key."
    )]
    async fn create_shipping_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateShippingProviderAccountParams>,
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
            .shipping_provider_administration
            .create(CreateShippingProviderAccountInput {
                actor,
                store_id,
                provider: params.provider,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                origin: shipping_address(params.origin),
                enabled: params.enabled,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(provider_account_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a shipping provider account in the selected Store. \
                        Requires confirm: true and an idempotency_key."
    )]
    async fn update_shipping_provider_account(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateShippingProviderAccountParams>,
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
        let id = match parse_uuid_field(&params.provider_account_id, "provider_account_id") {
            Ok(id) => ShippingProviderAccountId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .shipping_provider_administration
            .update(UpdateShippingProviderAccountInput {
                actor,
                store_id,
                id,
                display_name: params.display_name,
                credential_secret_reference: params.credential_secret_reference,
                origin: shipping_address(params.origin),
                enabled: params.enabled,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(provider_account_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Quote shipping rates for a fulfillment's parcel via a shipping provider \
                        account in the selected Store. Requires confirm: true and \
                        an idempotency_key."
    )]
    async fn quote_shipping_rates(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<QuoteShippingRatesParams>,
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
        let fulfillment_id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let provider_account_id =
            match parse_uuid_field(&params.provider_account_id, "provider_account_id") {
                Ok(id) => ShippingProviderAccountId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .shipping_provider_administration
            .quote_rates(QuoteShippingRatesInput {
                actor,
                store_id,
                fulfillment_id,
                provider_account_id,
                parcel: ShippingParcel {
                    length_millimetres: params.length_millimetres,
                    width_millimetres: params.width_millimetres,
                    height_millimetres: params.height_millimetres,
                    weight_grams: params.weight_grams,
                },
                now,
                idempotency,
            })
            .await
        {
            Ok(rates) => Ok(text_result(json!({
                "items": rates.into_iter().map(rate_quote_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Purchase a shipping label for a fulfillment using a prior rate quote, in \
                        the selected Store. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn purchase_shipping_label(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PurchaseShippingLabelParams>,
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
        let fulfillment_id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let rate_quote_id = match parse_uuid_field(&params.rate_quote_id, "rate_quote_id") {
            Ok(id) => ShippingRateQuoteId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .shipping_provider_administration
            .purchase_label(PurchaseShippingLabelInput {
                actor,
                store_id,
                fulfillment_id,
                rate_quote_id,
                now,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(shipping_label_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Cancel a purchased shipping label for a fulfillment, in the selected Store. Requires confirm: true and an idempotency_key."
    )]
    async fn cancel_shipping_label(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CancelShippingLabelParams>,
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
        let fulfillment_id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .shipping_provider_administration
            .cancel_label(CancelPurchasedShippingLabelInput {
                actor,
                store_id,
                fulfillment_id,
                now,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(shipping_label_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

impl ChaosMcp {
    async fn change_shipping_service_status(
        &self,
        parts: http::request::Parts,
        params: ChangeShippingServiceStatusParams,
        status: ShippingServiceStatus,
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
        let service_id = match parse_uuid_field(&params.shipping_service_id, "shipping_service_id")
        {
            Ok(id) => ShippingServiceId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .shipping_management
            .change_status(ChangeShippingServiceStatusInput {
                actor,
                store_id,
                service_id,
                status,
                idempotency,
            })
            .await
        {
            Ok(detail) => Ok(text_result(shipping_service_json(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn fulfillment_json(detail: FulfillmentDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "order_id": detail.order_id.as_uuid(),
        "status": detail.status.as_str(),
        "carrier": detail.carrier,
        "tracking_number": detail.tracking_number,
        "allocations": detail.allocations.into_iter().map(|allocation| json!({
            "product_variant_id": allocation.product_variant_id.as_uuid(),
            "quantity": allocation.quantity,
        })).collect::<Vec<_>>(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn return_json(detail: ReturnDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "order_id": detail.order_id.as_uuid(),
        "status": detail.status.as_str(),
        "lines": detail.lines.into_iter().map(|line| json!({
            "product_variant_id": line.product_variant_id.as_uuid(),
            "quantity": line.quantity,
        })).collect::<Vec<_>>(),
        "refund_id": detail.refund_id.map(|id| id.as_uuid()),
        "refund_amount_minor": detail.refund_amount_minor,
        "currency": detail.currency.as_str(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn shipping_service_json(detail: ShippingServiceDetail) -> serde_json::Value {
    let service = &detail.service;
    json!({
        "id": service.id().as_uuid(),
        "code": service.code(),
        "name": service.name(),
        "amount_minor": service.rate().amount_minor(),
        "currency": service.rate().currency().as_str(),
        "estimated_min_days": service.estimated_min_days(),
        "estimated_max_days": service.estimated_max_days(),
        "destination_countries": service.destination_countries(),
        "status": service.status().as_str(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn provider_account_json(detail: ShippingProviderAccountDetail) -> serde_json::Value {
    let account = &detail.account;
    json!({
        "id": account.id().as_uuid(),
        "provider": account.provider(),
        "display_name": account.display_name(),
        "enabled": account.enabled(),
        "credentials_configured": detail.credentials_configured,
        "origin": origin_json(&detail.origin),
        "credential_rotation_expires_at": detail.credential_rotation_expires_at.map(format_time),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn origin_json(origin: &ShippingAddress) -> serde_json::Value {
    json!({
        "name": origin.name,
        "company": origin.company,
        "address_line_1": origin.address_line_1,
        "address_line_2": origin.address_line_2,
        "city": origin.city,
        "region": origin.region,
        "postal_code": origin.postal_code,
        "country_code": origin.country_code,
        "phone": origin.phone,
        "email": origin.email,
    })
}

fn rate_quote_json(detail: ShippingRateQuoteDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "quote_request_id": detail.quote_request_id.as_uuid(),
        "carrier": detail.rate.carrier,
        "service": detail.rate.service,
        "amount_minor": detail.rate.amount_minor,
        "currency": detail.rate.currency.as_str(),
        "estimated_delivery_days": detail.rate.estimated_delivery_days,
        "guaranteed": detail.rate.guaranteed,
        "expires_at": format_time(detail.expires_at),
    })
}

fn shipping_label_json(detail: ShippingLabelDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "fulfillment_id": detail.fulfillment_id.as_uuid(),
        "rate_quote_id": detail.rate_quote_id.as_uuid(),
        "provider": detail.provider,
        "carrier": detail.label.carrier,
        "tracking_number": detail.label.tracking_number,
        "label_url": detail.label.label_url,
        "label_media_type": detail.label.label_media_type,
        "cancellation_status": detail.cancellation_status.map(cancellation_status_text),
        "purchased_at": format_time(detail.purchased_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn cancellation_status_text(
    status: chaos_application::ports::ShippingCancellationStatus,
) -> &'static str {
    use chaos_application::ports::ShippingCancellationStatus as Status;
    match status {
        Status::Submitted => "submitted",
        Status::Cancelled => "cancelled",
        Status::Rejected => "rejected",
        Status::NotAvailable => "not_available",
    }
}

fn shipping_address(params: ShippingOriginParams) -> ShippingAddress {
    ShippingAddress {
        name: params.name,
        company: params.company,
        address_line_1: params.address_line_1,
        address_line_2: params.address_line_2,
        city: params.city,
        region: params.region,
        postal_code: params.postal_code,
        country_code: params.country_code,
        phone: params.phone,
        email: params.email,
    }
}

fn parse_allocations(
    params: &[FulfillmentAllocationParams],
) -> Result<Vec<FulfillmentAllocationInput>, CallToolResult> {
    params
        .iter()
        .map(|allocation| {
            parse_uuid_field(&allocation.product_variant_id, "product_variant_id").map(|id| {
                FulfillmentAllocationInput {
                    product_variant_id: ProductVariantId::from_uuid(id),
                    quantity: allocation.quantity,
                }
            })
        })
        .collect()
}

fn parse_return_lines(params: &[ReturnLineParams]) -> Result<Vec<ReturnLineInput>, CallToolResult> {
    params
        .iter()
        .map(|line| {
            parse_uuid_field(&line.product_variant_id, "product_variant_id").map(|id| {
                ReturnLineInput {
                    product_variant_id: ProductVariantId::from_uuid(id),
                    quantity: line.quantity,
                }
            })
        })
        .collect()
}

fn parse_receipt(
    params: &[ReturnReceiptParams],
) -> Result<Vec<ReturnReceiptInput>, CallToolResult> {
    params
        .iter()
        .map(|receipt| {
            let product_variant_id =
                parse_uuid_field(&receipt.product_variant_id, "product_variant_id")?;
            let disposition = match receipt.disposition.as_str() {
                "restock" => ReturnDisposition::Restock,
                "discard" => ReturnDisposition::Discard,
                _ => {
                    return Err(CallToolResult::structured_error(json!({
                        "code": "invalid_params",
                        "message": "disposition must be \"restock\" or \"discard\"",
                    })));
                }
            };
            let inventory_location_id = match receipt.inventory_location_id.as_deref() {
                Some(value) => Some(InventoryLocationId::from_uuid(parse_uuid_field(
                    value,
                    "inventory_location_id",
                )?)),
                None => None,
            };
            Ok(ReturnReceiptInput {
                product_variant_id: ProductVariantId::from_uuid(product_variant_id),
                disposition,
                inventory_location_id,
            })
        })
        .collect()
}

fn parse_fulfillment_status(value: &str) -> Result<FulfillmentStatus, CallToolResult> {
    match value {
        "pending" => Ok(FulfillmentStatus::Pending),
        "shipped" => Ok(FulfillmentStatus::Shipped),
        "delivered" => Ok(FulfillmentStatus::Delivered),
        "cancelled" => Ok(FulfillmentStatus::Cancelled),
        _ => Err(CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": "target_status must be one of: pending, shipped, delivered, cancelled",
        }))),
    }
}

fn parse_return_status(value: &str) -> Result<ReturnStatus, CallToolResult> {
    match value {
        "requested" => Ok(ReturnStatus::Requested),
        "authorized" => Ok(ReturnStatus::Authorized),
        "rejected" => Ok(ReturnStatus::Rejected),
        "received" => Ok(ReturnStatus::Received),
        "completed" => Ok(ReturnStatus::Completed),
        _ => Err(CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": "target_status must be one of: requested, authorized, rejected, received, completed",
        }))),
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
