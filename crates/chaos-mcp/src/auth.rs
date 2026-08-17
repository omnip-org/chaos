use chaos_application::{
    ApplicationError,
    merchant::ApiKeyAuthentication,
    ports::{AdminActor, MachineActor},
};
use chaos_domain::merchant::ApiKeyScope;
use rmcp::model::CallToolResult;
use secrecy::SecretString;

use crate::error::tool_error;

/// Every MCP tool call authenticates independently against the `Authorization`
/// header on that HTTP request, mirroring the Store API's Bearer-token model.
/// `mcp:tools` is required on every call per ADR 0004; `scope` is the
/// tool-specific capability (e.g. `products:write`) checked in the same call.
///
/// Returns `Ok(Err(CallToolResult::error))` (not `Err(ErrorData)`) on auth
/// failure so the caller's MCP client renders "wrong scope"/"unauthorized" as
/// readable tool output rather than an opaque protocol error.
pub async fn authenticate_machine(
    api_key_authentication: &ApiKeyAuthentication,
    parts: &http::request::Parts,
    scope: ApiKeyScope,
) -> Result<AdminActor, CallToolResult> {
    let token = bearer_token(parts).map_err(tool_error)?;
    let actor: MachineActor = api_key_authentication
        .authenticate(&token, &[ApiKeyScope::McpTools, scope])
        .await
        .map_err(tool_error)?;
    Ok(AdminActor::Machine(actor))
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
