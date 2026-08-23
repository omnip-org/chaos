use rmcp::model::CallToolResult;
use serde_json::json;

/// Every write tool requires the caller to set `confirm: true` explicitly —
/// this is the entire "confirmation semantics" ADR 0004 asks for: no default,
/// no implicit yes, checked before any use-case call.
pub fn require_confirmation(confirm: bool) -> Result<(), CallToolResult> {
    if confirm {
        Ok(())
    } else {
        Err(CallToolResult::structured_error(json!({
            "code": "confirmation_required",
            "message": "confirm must be set to true to perform this operation. This action \
                         affects live store data; review the target resource with the \
                         corresponding read tool before confirming.",
        })))
    }
}
