mod catalog;
mod integrations;
mod operations;
mod pricing;
mod store;

use std::sync::Arc;

use chaos_application::{
    analytics::AnalyticsAdministration,
    catalog::{
        CatalogLocalization, CatalogManagement, CatalogQueries, CollectionAdministration,
        CreateProduct, MediaAdministration, ReviewAdministration,
    },
    fulfillment::{FulfillmentManagement, ShippingManagement, ShippingProviderAdministration},
    identity::AccessKeyAuthentication,
    inventory::InventoryManagement,
    payments::{PaymentProviderAdministration, PaymentService},
    ports::{AdminActor, Clock},
    pricing::{CreatePriceList, PricingManagement, PromotionManagement, TaxManagement},
    sales::OrderManagement,
    store::{
        CreateStore, ProviderSecretManagement, PublishableKeyManagement, StoreAdministration,
        StoreMembershipManagement, StoreQueries,
    },
};
use rmcp::{handler::server::router::tool::ToolRouter, model::CallToolResult, tool_handler};

/// Shared handles to the application-layer use cases the MCP surface calls.
/// Mirrors `ApiState` in `chaos-api`, but scoped to only what MCP tools need.
#[derive(Clone)]
pub struct McpState {
    pub public_base_url: String,
    pub access_key_authentication: Arc<AccessKeyAuthentication>,
    pub store_queries: Arc<StoreQueries>,
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
    pub publishable_key_management: Arc<PublishableKeyManagement>,
    pub provider_secret_management: Arc<ProviderSecretManagement>,
    pub analytics_administration: Arc<AnalyticsAdministration>,
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
            tool_router: Self::tool_router(),
        }
    }

    fn tool_router() -> ToolRouter<ChaosMcp> {
        let mut router = ToolRouter::new();
        for capability_router in Self::capability_tool_routers() {
            router.merge(capability_router);
        }
        router
    }

    fn capability_tool_routers() -> [ToolRouter<ChaosMcp>; 18] {
        [
            Self::products_tool_router(),
            Self::stores_tool_router(),
            Self::price_lists_tool_router(),
            Self::inventory_tool_router(),
            Self::orders_tool_router(),
            Self::collections_tool_router(),
            Self::promotions_tool_router(),
            Self::tax_rules_tool_router(),
            Self::fulfillment_tool_router(),
            Self::store_admin_tool_router(),
            Self::payments_tool_router(),
            Self::payment_providers_tool_router(),
            Self::media_tool_router(),
            Self::localization_tool_router(),
            Self::reviews_tool_router(),
            Self::publishable_keys_tool_router(),
            Self::analytics_tool_router(),
            Self::provider_secrets_tool_router(),
        ]
    }

    async fn store_actor(
        &self,
        parts: &http::request::Parts,
    ) -> Result<chaos_application::store::StoreActor, CallToolResult> {
        match crate::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            parts,
        )
        .await?
        {
            AdminActor::Store(actor) => Ok(actor),
            AdminActor::Machine(_) => unreachable!("MCP authentication returns a User actor"),
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
             Authorization: Bearer <access-key> header using a user-owned Access Key. Every \
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

#[cfg(test)]
mod tests {
    use super::ChaosMcp;

    #[test]
    fn every_capability_tool_router_is_in_the_aggregate_router() {
        let capability_routers = ChaosMcp::capability_tool_routers();
        let expected_route_count = capability_routers
            .iter()
            .map(|router| router.map.len())
            .sum::<usize>();
        let aggregate_router = ChaosMcp::tool_router();

        assert_eq!(aggregate_router.map.len(), expected_route_count);
        for capability_router in capability_routers {
            for name in capability_router.map.keys() {
                assert!(aggregate_router.has_route(name));
            }
        }
    }
}
