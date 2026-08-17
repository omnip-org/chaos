use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::tools::ChaosMcp;
pub use crate::tools::McpState;

/// Mounts the MCP Streamable HTTP surface. Sessions are held in-process
/// (`LocalSessionManager`) since every tool call re-authenticates against its
/// own `Authorization` header rather than relying on session-bound identity.
pub fn router(state: McpState) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(ChaosMcp::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    Router::new().fallback_service(service)
}
