use chaos_application::{
    ApplicationError,
    identity::McpKeyAuthentication,
    merchant::MerchantQueries,
    ports::{AdminActor, McpPrincipal},
};
use chaos_domain::merchant::StoreId;
use rmcp::model::CallToolResult;
use secrecy::SecretString;

use crate::error::tool_error;

/// Every MCP tool call authenticates a user-owned key and authorizes the user
/// against the Store selected by `X-Chaos-Store-Id`. Membership is checked on
/// every call so leaving a Store takes effect without rotating the key.
///
/// Returns `Ok(Err(CallToolResult::error))` (not `Err(ErrorData)`) on auth
/// failure so the caller's MCP client renders "wrong scope"/"unauthorized" as
/// readable tool output rather than an opaque protocol error.
pub async fn authenticate_mcp(
    mcp_key_authentication: &McpKeyAuthentication,
    merchant_queries: &MerchantQueries,
    parts: &http::request::Parts,
) -> Result<AdminActor, CallToolResult> {
    let principal = authenticate_principal(mcp_key_authentication, parts).await?;
    let store_id = store_id(parts).map_err(tool_error)?;
    let actor = merchant_queries
        .authorize(principal.user_id, store_id)
        .await
        .map_err(tool_error)?
        .with_mcp_key(principal.key_id);
    tracing::info!(
        request_id = request_id(parts),
        mcp_key_id = %principal.key_id.as_uuid(),
        user_id = %principal.user_id.as_uuid(),
        store_id = %store_id.as_uuid(),
        "MCP request authorized"
    );
    Ok(AdminActor::Store(actor))
}

pub async fn authenticate_principal(
    mcp_key_authentication: &McpKeyAuthentication,
    parts: &http::request::Parts,
) -> Result<McpPrincipal, CallToolResult> {
    let token = bearer_token(parts).map_err(tool_error)?;
    let principal = mcp_key_authentication
        .authenticate(&token)
        .await
        .map_err(tool_error)?;
    tracing::info!(
        request_id = request_id(parts),
        mcp_key_id = %principal.key_id.as_uuid(),
        user_id = %principal.user_id.as_uuid(),
        "MCP key authenticated"
    );
    Ok(principal)
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

fn store_id(parts: &http::request::Parts) -> Result<StoreId, ApplicationError> {
    let value = parts
        .headers
        .get("x-chaos-store-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "X-Chaos-Store-Id",
                reason: "must be a valid Store UUID".into(),
            }],
        })?;
    Ok(StoreId::from_uuid(value))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chaos_application::{
        identity::McpKeyAuthentication,
        merchant::MerchantQueries,
        ports::{
            GeneratedMcpKeyMaterial, McpKeyListItem, McpKeyRepository, McpPrincipal,
            MerchantReadRepository, StoreListItem,
        },
    };
    use chaos_domain::{
        identity::{McpKey, McpKeyId, UserId},
        merchant::{StoreId, StoreRole},
    };
    use secrecy::SecretString;

    use super::*;

    struct FixedMcpKeys(McpPrincipal);

    #[async_trait]
    impl McpKeyRepository for FixedMcpKeys {
        async fn create(
            &self,
            _key: &McpKey,
            _material: &GeneratedMcpKeyMaterial,
        ) -> Result<(), ApplicationError> {
            unreachable!()
        }

        async fn list(
            &self,
            _user_id: UserId,
            _after: Option<McpKeyId>,
            _limit: u16,
        ) -> Result<Vec<McpKeyListItem>, ApplicationError> {
            unreachable!()
        }

        async fn revoke(
            &self,
            _user_id: UserId,
            _key_id: McpKeyId,
        ) -> Result<(), ApplicationError> {
            unreachable!()
        }

        async fn authenticate(
            &self,
            _presented_key: &SecretString,
        ) -> Result<Option<McpPrincipal>, ApplicationError> {
            Ok(Some(self.0))
        }
    }

    struct FixedMembership {
        user_id: UserId,
        store_id: StoreId,
    }

    #[async_trait]
    impl MerchantReadRepository for FixedMembership {
        async fn membership_role(
            &self,
            user_id: UserId,
            store_id: StoreId,
        ) -> Result<Option<StoreRole>, ApplicationError> {
            Ok((user_id == self.user_id && store_id == self.store_id).then_some(StoreRole::Owner))
        }

        async fn list_stores(
            &self,
            _user_id: UserId,
            _after: Option<StoreId>,
            _limit: u16,
        ) -> Result<Vec<StoreListItem>, ApplicationError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn resolves_a_user_key_then_rechecks_the_selected_store_membership() {
        let user_id = UserId::new();
        let key_id = McpKeyId::new();
        let store_id = StoreId::new();
        let authentication =
            McpKeyAuthentication::new(Arc::new(FixedMcpKeys(McpPrincipal { key_id, user_id })));
        let queries = MerchantQueries::new(Arc::new(FixedMembership { user_id, store_id }));
        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer cc_mcp_v1_test_secret")
            .header("x-chaos-store-id", store_id.as_uuid().to_string())
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();

        let actor = authenticate_mcp(&authentication, &queries, &parts)
            .await
            .unwrap();
        assert_eq!(actor.store_id(), store_id);
        assert_eq!(actor.audit_user_id(), user_id);
        let AdminActor::Store(store_actor) = actor else {
            unreachable!()
        };
        assert_eq!(store_actor.mcp_key_id(), Some(key_id));
    }

    #[tokio::test]
    async fn rejects_a_store_without_current_membership() {
        let user_id = UserId::new();
        let allowed_store_id = StoreId::new();
        let authentication = McpKeyAuthentication::new(Arc::new(FixedMcpKeys(McpPrincipal {
            key_id: McpKeyId::new(),
            user_id,
        })));
        let queries = MerchantQueries::new(Arc::new(FixedMembership {
            user_id,
            store_id: allowed_store_id,
        }));
        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer cc_mcp_v1_test_secret")
            .header("x-chaos-store-id", StoreId::new().as_uuid().to_string())
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();

        assert!(
            authenticate_mcp(&authentication, &queries, &parts)
                .await
                .is_err()
        );
    }
}
