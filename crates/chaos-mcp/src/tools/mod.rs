mod collections;
mod inventory;
mod orders;
mod price_lists;
mod products;

use std::sync::Arc;

use chaos_application::{
    catalog::{CatalogManagement, CatalogQueries, CollectionAdministration, CreateProduct},
    inventory::InventoryManagement,
    merchant::ApiKeyAuthentication,
    ports::Clock,
    pricing::{CreatePriceList, PricingManagement},
    sales::OrderManagement,
};
use rmcp::{handler::server::router::tool::ToolRouter, tool_handler};

/// Shared handles to the application-layer use cases the MCP surface calls.
/// Mirrors `ApiState` in `chaos-api`, but scoped to only what MCP tools need.
#[derive(Clone)]
pub struct McpState {
    pub api_key_authentication: Arc<ApiKeyAuthentication>,
    pub catalog_queries: Arc<CatalogQueries>,
    pub create_product: Arc<CreateProduct>,
    pub catalog_management: Arc<CatalogManagement>,
    pub collection_administration: Arc<CollectionAdministration>,
    pub pricing_management: Arc<PricingManagement>,
    pub create_price_list: Arc<CreatePriceList>,
    pub inventory_management: Arc<InventoryManagement>,
    pub order_management: Arc<OrderManagement>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct ChaosMcp {
    pub(crate) state: McpState,
    tool_router: ToolRouter<ChaosMcp>,
}

impl ChaosMcp {
    pub fn new(state: McpState) -> Self {
        Self {
            state,
            tool_router: Self::products_tool_router()
                + Self::price_lists_tool_router()
                + Self::inventory_tool_router()
                + Self::orders_tool_router()
                + Self::collections_tool_router(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for ChaosMcp {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let mut info = rmcp::model::ServerInfo::default();
        info.capabilities = rmcp::model::ServerCapabilities::builder()
            .enable_tools()
            .build();
        info.instructions = Some(
            "Chaos Commerce admin tools. Every tool call authenticates against the \
             Authorization: Bearer <api-key> header using a Store-scoped Secret API key \
             with the mcp:tools scope plus the tool-specific scope. Read tools return \
             store data; write tools require confirm: true and a client-chosen \
             idempotency_key."
                .into(),
        );
        info
    }
}
