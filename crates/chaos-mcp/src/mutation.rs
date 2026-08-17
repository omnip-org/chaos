use chaos_application::ports::IdempotencyRequest;
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

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

/// Builds the `IdempotencyRequest` for a write tool call: the caller-supplied
/// `idempotency_key` identifies the attempt, and the server derives the
/// fingerprint from the full validated input so a replay with different
/// arguments under the same key is detected as a conflict rather than
/// silently replayed.
pub fn idempotency_request(key: String, params: &impl Serialize) -> IdempotencyRequest {
    let fingerprint_source =
        serde_json::to_vec(params).expect("tool params are always JSON-serializable");
    IdempotencyRequest {
        key,
        request_fingerprint: Sha256::digest(fingerprint_source).into(),
    }
}
