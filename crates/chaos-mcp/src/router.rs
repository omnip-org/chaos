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
pub fn router(state: McpState, allowed_hosts: Vec<String>) -> Router {
    // Every request carries its own MCP protocol context and is authenticated
    // independently, so it can land on any API replica without sticky sessions.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts);
    let service = StreamableHttpService::new(
        move || Ok(ChaosMcp::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new().fallback_service(service)
}
