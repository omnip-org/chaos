mod catalog;
mod integrations;
mod operations;
mod params;
mod pricing;
mod store;

pub(crate) use params::StoreIdParams;

use chaos_core::contracts::AdminActor;
use rmcp::{handler::server::router::tool::ToolRouter, model::CallToolResult, tool_handler};

use crate::http::ApiState;

#[derive(Clone)]
pub struct ChaosMcp {
    pub(crate) state: ApiState,
    tool_router: ToolRouter<ChaosMcp>,
}

impl ChaosMcp {
    pub fn new(state: ApiState) -> Self {
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

    fn capability_tool_routers() -> [ToolRouter<ChaosMcp>; 15] {
        [
            Self::products_tool_router(),
            Self::stores_tool_router(),
            Self::price_lists_tool_router(),
            Self::inventory_tool_router(),
            Self::orders_tool_router(),
            Self::collections_tool_router(),
            Self::store_admin_tool_router(),
            Self::payments_tool_router(),
            Self::payment_providers_tool_router(),
            Self::media_tool_router(),
            Self::reviews_tool_router(),
            Self::publishable_keys_tool_router(),
            Self::analytics_tool_router(),
            Self::provider_secrets_tool_router(),
            Self::fulfillment_tool_router(),
        ]
    }

    async fn store_actor(
        &self,
        parts: &http::request::Parts,
        store_id: &str,
    ) -> Result<chaos_core::store::StoreActor, CallToolResult> {
        match crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            parts,
            store_id,
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
             Authorization: Bearer header using an MCP OAuth access token or a legacy user-owned \
             Access Key. Every Store-scoped tool must include its explicit store_id parameter, \
             and current Store membership is checked before the tool runs. create_store and \
             list_stores are User-scoped and do not require store_id. Read tools return store \
             data; write tools require confirm: true."
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
        assert!(aggregate_router.has_route("prepare_media_upload"));
        assert!(aggregate_router.has_route("refresh_media_upload"));
        assert!(aggregate_router.has_route("complete_media_upload"));
        assert!(aggregate_router.has_route("get_media_asset"));
        assert!(aggregate_router.has_route("archive_media_asset"));
        assert!(aggregate_router.has_route("attach_product_media"));
        assert!(aggregate_router.has_route("attach_review_media"));
        assert!(aggregate_router.has_route("attach_product_meta_media"));
        assert!(aggregate_router.has_route("list_product_meta_media"));
        assert!(aggregate_router.has_route("archive_product_meta_media"));
        assert!(aggregate_router.has_route("list_review_media"));
        assert!(aggregate_router.has_route("archive_review_media"));
        assert!(aggregate_router.has_route("create_manual_review"));
        assert!(!aggregate_router.has_route("prepare_product_media_upload"));
        assert!(!aggregate_router.has_route("prepare_review_media_upload"));
        assert!(!aggregate_router.has_route("upload_product_media"));
        for capability_router in capability_routers {
            for name in capability_router.map.keys() {
                assert!(aggregate_router.has_route(name));
            }
        }
    }
}
