use chaos_application::pricing::{
    ChangePriceListStatusInput, CreatePriceInput, CreatePriceListInput, UpdatePriceListInput,
};
use chaos_domain::{catalog::ProductVariantId, pricing::PriceListId};
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

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, JsonSchema)]
pub struct ListPriceListsParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of price lists to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetPriceListParams {
    /// The price list's UUID.
    pub price_list_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PriceEntryParams {
    /// The priced product variant's UUID.
    pub product_variant_id: String,
    /// The price in the price list's smallest currency unit (e.g. cents for USD).
    pub amount_minor: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreatePriceListParams {
    /// URL-safe code, unique within the Store.
    pub code: String,
    pub name: String,
    /// Three-letter ISO 4217 currency code (e.g. USD).
    pub currency: String,
    /// RFC 3339 timestamp; omit for no start boundary.
    #[serde(default)]
    pub starts_at: Option<String>,
    /// RFC 3339 timestamp; omit for no end boundary.
    #[serde(default)]
    pub ends_at: Option<String>,
    /// Activate immediately on creation. Every priced variant must already be active.
    #[serde(default)]
    pub activate: bool,
    pub prices: Vec<PriceEntryParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdatePriceListParams {
    /// The price list's UUID.
    pub price_list_id: String,
    pub code: String,
    pub name: String,
    pub currency: String,
    #[serde(default)]
    pub starts_at: Option<String>,
    #[serde(default)]
    pub ends_at: Option<String>,
    /// The complete replacement set of prices for this list.
    pub prices: Vec<PriceEntryParams>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangePriceListStatusParams {
    /// The price list's UUID.
    pub price_list_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = price_lists_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List price lists in the selected Store, including draft \
                        and archived lists. Paginated; use the returned next_cursor for more pages."
    )]
    async fn list_price_lists(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListPriceListsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
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
        let after = match params.cursor.as_deref().map(parse_price_list_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .pricing_management
            .list(actor, store_id, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "id": item.id.as_uuid(),
                            "code": item.code,
                            "name": item.name,
                            "currency": item.currency.as_str(),
                            "status": item.status.as_str(),
                            "starts_at": item.starts_at.map(format_time),
                            "ends_at": item.ends_at.map(format_time),
                            "price_count": item.price_count,
                            "created_at": format_time(item.created_at),
                            "updated_at": format_time(item.updated_at),
                        })
                    })
                    .collect::<Vec<_>>();
                let next_cursor = page
                    .has_more
                    .then(|| {
                        items
                            .last()
                            .and_then(|item| item["id"].as_str().map(String::from))
                    })
                    .flatten();
                Ok(text_result(json!({
                    "items": items,
                    "has_more": page.has_more,
                    "next_cursor": next_cursor,
                })))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get full details for a single price list in the selected Store, \
                        including every priced variant and its amount."
    )]
    async fn get_price_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetPriceListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
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
        let price_list_id = match parse_uuid_field(&params.price_list_id, "price_list_id") {
            Ok(id) => PriceListId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .pricing_management
            .get(actor, store_id, price_list_id)
            .await
        {
            Ok(detail) => Ok(text_result(json!({
                "id": detail.item.id.as_uuid(),
                "code": detail.item.code,
                "name": detail.item.name,
                "currency": detail.item.currency.as_str(),
                "status": detail.item.status.as_str(),
                "starts_at": detail.item.starts_at.map(format_time),
                "ends_at": detail.item.ends_at.map(format_time),
                "created_at": format_time(detail.item.created_at),
                "updated_at": format_time(detail.item.updated_at),
                "prices": detail.prices.into_iter().map(|price| json!({
                    "product_variant_id": price.product_variant_id.as_uuid(),
                    "amount_minor": price.amount_minor,
                })).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a price list in the selected Store. Set activate: \
                        true to activate immediately (every priced variant must already be \
                        active). Requires confirm: true and an idempotency_key."
    )]
    async fn create_price_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreatePriceListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
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
        let starts_at = match parse_optional_time("starts_at", params.starts_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let ends_at = match parse_optional_time("ends_at", params.ends_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let prices = match parse_prices(&params.prices) {
            Ok(prices) => prices,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .create_price_list
            .execute(CreatePriceListInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                currency: params.currency,
                starts_at,
                ends_at,
                activate: params.activate,
                prices,
                idempotency,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({ "id": output.price_list_id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Replace a price list's code, name, currency, schedule, and full price \
                        set in the selected Store. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn update_price_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdatePriceListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
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
        let price_list_id = match parse_uuid_field(&params.price_list_id, "price_list_id") {
            Ok(id) => PriceListId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let starts_at = match parse_optional_time("starts_at", params.starts_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let ends_at = match parse_optional_time("ends_at", params.ends_at.as_deref()) {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let prices = match parse_prices(&params.prices) {
            Ok(prices) => prices,
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .pricing_management
            .update(UpdatePriceListInput {
                actor,
                store_id,
                price_list_id,
                code: params.code,
                name: params.name,
                currency: params.currency,
                starts_at,
                ends_at,
                prices,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a draft price list in the selected Store. Every \
                        priced variant must already be active. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn activate_price_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangePriceListStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_price_list_status(parts, params, true).await
    }

    #[tool(
        description = "Archive a price list in the selected Store, removing it \
                        from pricing resolution without deleting it. Requires confirm: true and \
                        an idempotency_key."
    )]
    async fn archive_price_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangePriceListStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_price_list_status(parts, params, false).await
    }
}

impl ChaosMcp {
    async fn change_price_list_status(
        &self,
        parts: http::request::Parts,
        params: ChangePriceListStatusParams,
        activate: bool,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
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
        let price_list_id = match parse_uuid_field(&params.price_list_id, "price_list_id") {
            Ok(id) => PriceListId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        let input = ChangePriceListStatusInput {
            actor,
            store_id,
            price_list_id,
            idempotency,
        };
        let result = if activate {
            self.state.pricing_management.activate(input).await
        } else {
            self.state.pricing_management.archive(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_prices(params: &[PriceEntryParams]) -> Result<Vec<CreatePriceInput>, CallToolResult> {
    params
        .iter()
        .map(|price| {
            parse_uuid_field(&price.product_variant_id, "product_variant_id").map(|id| {
                CreatePriceInput {
                    product_variant_id: ProductVariantId::from_uuid(id),
                    amount_minor: price.amount_minor,
                }
            })
        })
        .collect()
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

fn parse_price_list_cursor(value: &str) -> Result<PriceListId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(PriceListId::from_uuid)
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
