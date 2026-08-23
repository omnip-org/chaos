mod catalog;
mod integrations;
mod operations;
mod pricing;
mod store;

use chaos_core::ports::AdminActor;
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

    fn capability_tool_routers() -> [ToolRouter<ChaosMcp>; 14] {
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
        ]
    }

    async fn store_actor(
        &self,
        parts: &http::request::Parts,
    ) -> Result<chaos_core::store::StoreActor, CallToolResult> {
        match crate::mcp::auth::authenticate_mcp(
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
             store data; write tools require confirm: true."
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
