use chaos_core::{ApplicationError, contracts::AdminActor, store::StoreQueries};
use chaos_domain::{identity::UserId, store::StoreId};
use rmcp::model::CallToolResult;
use secrecy::SecretString;

use crate::mcp::McpOAuthService;
use crate::mcp::error::tool_error;

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedMcpPrincipal {
    pub user_id: UserId,
}

/// Every MCP tool call authenticates an OAuth access token and authorizes the
/// user against the Store selected by the tool's explicit `store_id` input.
/// Membership is checked on every call so leaving a Store takes effect without
/// rotating an OAuth client token.
pub async fn authenticate_mcp(
    oauth: &McpOAuthService,
    store_queries: &StoreQueries,
    parts: &http::request::Parts,
    requested_store_id: &str,
) -> Result<AdminActor, CallToolResult> {
    let principal = authenticate_principal(oauth, parts).await?;
    let store_id = parse_store_id(requested_store_id).map_err(tool_error)?;
    let actor = store_queries
        .authorize(principal.user_id, store_id)
        .await
        .map_err(tool_error)?;
    tracing::info!(
        request_id = request_id(parts),
        user_id = %principal.user_id.as_uuid(),
        store_id = %store_id.as_uuid(),
        "MCP OAuth request authorized"
    );
    Ok(AdminActor::Store(actor))
}

pub async fn authenticate_principal(
    oauth: &McpOAuthService,
    parts: &http::request::Parts,
) -> Result<AuthenticatedMcpPrincipal, CallToolResult> {
    if let Some(principal) = parts.extensions.get::<AuthenticatedMcpPrincipal>() {
        return Ok(principal.clone());
    }
    let token = bearer_token(parts).map_err(tool_error)?;
    let principal = authenticate_token(oauth, &token)
        .await
        .map_err(tool_error)?;
    tracing::info!(
        request_id = request_id(parts),
        user_id = %principal.user_id.as_uuid(),
        "MCP OAuth token authenticated"
    );
    Ok(principal)
}

pub(crate) async fn authenticate_token(
    oauth: &McpOAuthService,
    token: &SecretString,
) -> Result<AuthenticatedMcpPrincipal, ApplicationError> {
    let principal = oauth.authenticate_access_token(token).await?;
    Ok(AuthenticatedMcpPrincipal {
        user_id: principal.user_id,
    })
}

fn request_id(parts: &http::request::Parts) -> &str {
    parts
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
}

fn bearer_token(parts: &http::request::Parts) -> Result<SecretString, ApplicationError> {
    let value = parts
        .headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

fn parse_store_id(value: &str) -> Result<StoreId, ApplicationError> {
    let value = uuid::Uuid::parse_str(value).map_err(|_| ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "store_id",
            reason: "must be a valid Store UUID".into(),
        }],
    })?;
    Ok(StoreId::from_uuid(value))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret as _;

    use super::*;

    #[test]
    fn bearer_token_requires_a_non_empty_bearer_value() {
        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer oauth_test_token")
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();
        assert_eq!(
            bearer_token(&parts).unwrap().expose_secret(),
            "oauth_test_token"
        );

        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer ")
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();
        assert!(matches!(
            bearer_token(&parts),
            Err(ApplicationError::Unauthorized)
        ));
    }
}
