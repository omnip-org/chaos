mod analytics;
mod api_keys;
mod collections;
mod fulfillment;
mod inventory;
mod localization;
mod media;
mod orders;
mod payment_providers;
mod payments;
mod price_lists;
mod products;
mod promotions;
mod provider_secrets;
mod reviews;
mod store_admin;
mod stores;
mod tax_rules;

use std::sync::Arc;

use chaos_application::{
    analytics::{
        AnalyticsAdministration, AnalyticsDestinations, AnalyticsPrivacy, AnalyticsReporting,
    },
    catalog::{
        CatalogLocalization, CatalogManagement, CatalogQueries, CollectionAdministration,
        CreateProduct, MediaAdministration, ReviewAdministration,
    },
    fulfillment::{FulfillmentManagement, ShippingManagement, ShippingProviderAdministration},
    identity::McpKeyAuthentication,
    inventory::InventoryManagement,
    merchant::{
        ApiKeyManagement, CreateStore, MerchantQueries, ProviderSecretManagement,
        StoreAdministration, StoreMembershipManagement,
    },
    payments::{PaymentProviderAdministration, PaymentService},
    ports::Clock,
    pricing::{CreatePriceList, PricingManagement, PromotionManagement, TaxManagement},
    sales::OrderManagement,
};
use rmcp::{handler::server::router::tool::ToolRouter, tool_handler};

/// Shared handles to the application-layer use cases the MCP surface calls.
/// Mirrors `ApiState` in `chaos-api`, but scoped to only what MCP tools need.
#[derive(Clone)]
pub struct McpState {
    pub mcp_key_authentication: Arc<McpKeyAuthentication>,
    pub merchant_queries: Arc<MerchantQueries>,
    pub store_membership_management: Arc<StoreMembershipManagement>,
    pub create_store: Arc<CreateStore>,
    pub catalog_queries: Arc<CatalogQueries>,
    pub create_product: Arc<CreateProduct>,
    pub catalog_management: Arc<CatalogManagement>,
    pub collection_administration: Arc<CollectionAdministration>,
    pub pricing_management: Arc<PricingManagement>,
    pub create_price_list: Arc<CreatePriceList>,
    pub promotion_management: Arc<PromotionManagement>,
    pub tax_management: Arc<TaxManagement>,
    pub inventory_management: Arc<InventoryManagement>,
    pub order_management: Arc<OrderManagement>,
    pub fulfillment_management: Arc<FulfillmentManagement>,
    pub shipping_management: Arc<ShippingManagement>,
    pub shipping_provider_administration: Arc<ShippingProviderAdministration>,
    pub store_administration: Arc<StoreAdministration>,
    pub payment_service: Arc<PaymentService>,
    pub payment_provider_administration: Arc<PaymentProviderAdministration>,
    pub media_administration: Arc<MediaAdministration>,
    pub catalog_localization: Arc<CatalogLocalization>,
    pub review_administration: Arc<ReviewAdministration>,
    pub api_key_management: Arc<ApiKeyManagement>,
    pub provider_secret_management: Arc<ProviderSecretManagement>,
    pub analytics_administration: Arc<AnalyticsAdministration>,
    pub analytics_privacy: Arc<AnalyticsPrivacy>,
    pub analytics_reporting: Arc<AnalyticsReporting>,
    pub analytics_destinations: Arc<AnalyticsDestinations>,
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
                + Self::stores_tool_router()
                + Self::price_lists_tool_router()
                + Self::inventory_tool_router()
                + Self::orders_tool_router()
                + Self::collections_tool_router()
                + Self::promotions_tool_router()
                + Self::tax_rules_tool_router()
                + Self::fulfillment_tool_router()
                + Self::store_admin_tool_router()
                + Self::payments_tool_router()
                + Self::payment_providers_tool_router()
                + Self::media_tool_router()
                + Self::localization_tool_router()
                + Self::reviews_tool_router()
                + Self::api_keys_tool_router()
                + Self::analytics_tool_router()
                + Self::provider_secrets_tool_router(),
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
             Authorization: Bearer <mcp-key> header using a user-owned MCP key. Every \
             Store-scoped request must include X-Chaos-Store-Id and current Store membership \
             is checked before the tool runs. create_store and list_stores are User-scoped and \
             do not require that header. Read tools return \
             store data; write tools require confirm: true and a client-chosen \
             idempotency_key."
                .into(),
        );
        info
    }
}
