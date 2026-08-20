use chaos_application::ApplicationError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::json;

/// Maps `ApplicationError` to a tool-level `CallToolResult::error`, per rmcp's
/// guidance: the caller's MCP client renders `CallToolResult` content, but
/// typically renders `Err(ErrorData)` opaquely. Every one of these is a case
/// the caller should be able to read and act on (wrong scope, bad input,
/// missing resource, etc.), so none of them should be a protocol error.
pub fn tool_error(error: ApplicationError) -> CallToolResult {
    let (code, message, details): (&'static str, String, Option<serde_json::Value>) = match error {
        ApplicationError::Validation { violations } => (
            "validation_failed",
            "one or more fields are invalid".into(),
            Some(json!(
                violations
                    .into_iter()
                    .map(|item| json!({ "field": item.field, "reason": item.reason }))
                    .collect::<Vec<_>>()
            )),
        ),
        ApplicationError::Unauthorized => {
            ("unauthorized", "authentication is required".into(), None)
        }
        ApplicationError::Forbidden => (
            "forbidden",
            "the authenticated User is not allowed to perform this operation in the selected Store"
                .into(),
            None,
        ),
        ApplicationError::NotFound { resource, id } => {
            ("not_found", format!("{resource} {id} was not found"), None)
        }
        ApplicationError::Conflict { code, message } => (code, message.into(), None),
        ApplicationError::RateLimited {
            retry_after_seconds,
        } => (
            "rate_limited",
            format!("retry after {retry_after_seconds} seconds"),
            None,
        ),
        ApplicationError::Unavailable { service, source } => {
            tracing::warn!(%service, error = %source, "application dependency unavailable");
            (
                "service_unavailable",
                "a required service is temporarily unavailable".into(),
                None,
            )
        }
        ApplicationError::Unexpected(source) => {
            tracing::error!(error = %source, "unexpected application error");
            (
                "internal_error",
                "an unexpected error occurred".into(),
                None,
            )
        }
    };
    let mut payload = json!({ "code": code, "message": message });
    if let Some(details) = details {
        payload["details"] = details;
    }
    CallToolResult::structured_error(payload)
}

pub fn text_result(value: serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}
