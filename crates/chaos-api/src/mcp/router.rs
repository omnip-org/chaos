use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::State,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use secrecy::SecretString;

use crate::http::ApiState;
use crate::mcp::tools::ChaosMcp;

/// Mounts the MCP Streamable HTTP surface. The transport is configured
/// stateless; `LocalSessionManager` is retained only as the rmcp service's
/// session-manager dependency, while every tool call re-authenticates its own
/// `Authorization` header.
pub fn router(state: ApiState) -> Router {
    // Every request carries its own MCP protocol context and is authenticated
    // independently, so it can land on any API replica without sticky sessions.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(state.mcp_allowed_hosts.clone())
        .with_allowed_origins(state.mcp_allowed_origins.clone());
    let service_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(ChaosMcp::new(service_state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .fallback_service(service)
        .layer(middleware::from_fn_with_state(
            state,
            authenticate_http_request,
        ))
}

async fn authenticate_http_request(
    State(state): State<ApiState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if !host_is_allowed(&request, &state.mcp_allowed_hosts) {
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: Host header is not allowed",
        )
            .into_response();
    }
    let Some(token) = bearer_token(request.headers()) else {
        return challenge(
            StatusCode::UNAUTHORIZED,
            &state,
            None,
            "authentication is required",
        );
    };
    match super::auth::authenticate_token(&state.mcp_oauth, &token).await {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(chaos_core::ApplicationError::Unauthorized) => challenge(
            StatusCode::UNAUTHORIZED,
            &state,
            Some("invalid_token"),
            "the bearer token is invalid or expired",
        ),
        Err(chaos_core::ApplicationError::Forbidden) => challenge(
            StatusCode::FORBIDDEN,
            &state,
            Some("insufficient_scope"),
            "the bearer token does not grant the mcp scope",
        ),
        Err(error) => {
            tracing::warn!(error = %error, "MCP authentication dependency failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "MCP authentication is temporarily unavailable",
            )
                .into_response()
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<SecretString> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .map(|value| SecretString::from(value.to_owned()))
}

fn challenge(status: StatusCode, state: &ApiState, error: Option<&str>, message: &str) -> Response {
    let mut value = format!(
        "Bearer resource_metadata=\"{}\"",
        state.mcp_oauth.protected_resource_metadata_endpoint()
    );
    if let Some(error) = error {
        value.push_str(&format!(", error=\"{error}\""));
    }
    if error == Some("insufficient_scope") {
        value.push_str(", scope=\"mcp\"");
    }
    let mut response = (
        status,
        axum::Json(serde_json::json!({
            "error": error.unwrap_or("unauthorized"),
            "error_description": message,
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&value) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn host_is_allowed(request: &Request<Body>, allowed_hosts: &[String]) -> bool {
    let Some(raw_host) = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| request.uri().authority().map(|value| value.as_str()))
    else {
        return false;
    };
    let (host, port) = normalize_authority(raw_host);
    if allowed_hosts.is_empty() {
        return true;
    }
    allowed_hosts.iter().any(|allowed| {
        let (allowed_host, allowed_port) = normalize_authority(allowed);
        host == allowed_host && (allowed_port.is_none() || allowed_port == port)
    })
}

fn normalize_authority(value: &str) -> (String, Option<u16>) {
    let value = value.trim();
    if let Ok(authority) = http::uri::Authority::try_from(value) {
        return (authority.host().to_ascii_lowercase(), authority.port_u16());
    }
    (value.trim_matches(['[', ']']).to_ascii_lowercase(), None)
}
