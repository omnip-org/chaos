use chaos_core::{
    ApplicationError,
    contracts::{AdminActor, McpPrincipal},
    identity::AccessKeyAuthentication,
    store::StoreQueries,
};
use chaos_domain::store::StoreId;
use rmcp::model::CallToolResult;
use secrecy::SecretString;

use crate::mcp::error::tool_error;

/// Every MCP tool call authenticates a user-owned key and authorizes the user
/// against the Store selected by the tool's explicit `store_id` parameter.
/// Membership is checked on every call so leaving a Store takes effect without
/// rotating the key. Store scope is deliberately part of the tool input rather
/// than an HTTP header so MCP clients can discover it from the tool schema.
///
/// Returns `Ok(Err(CallToolResult::error))` (not `Err(ErrorData)`) on auth
/// failure so the caller's MCP client renders "wrong scope"/"unauthorized" as
/// readable tool output rather than an opaque protocol error.
pub async fn authenticate_mcp(
    access_key_authentication: &AccessKeyAuthentication,
    store_queries: &StoreQueries,
    parts: &http::request::Parts,
    requested_store_id: &str,
) -> Result<AdminActor, CallToolResult> {
    let principal = authenticate_principal(access_key_authentication, parts).await?;
    let store_id = parse_store_id(requested_store_id).map_err(tool_error)?;
    let actor = store_queries
        .authorize(principal.user_id, store_id)
        .await
        .map_err(tool_error)?
        .with_access_key(principal.key_id);
    tracing::info!(
        request_id = request_id(parts),
        access_key_id = %principal.key_id.as_uuid(),
        user_id = %principal.user_id.as_uuid(),
        store_id = %store_id.as_uuid(),
        "MCP request authorized"
    );
    Ok(AdminActor::Store(actor))
}

pub async fn authenticate_principal(
    access_key_authentication: &AccessKeyAuthentication,
    parts: &http::request::Parts,
) -> Result<McpPrincipal, CallToolResult> {
    let token = bearer_token(parts).map_err(tool_error)?;
    let principal = access_key_authentication
        .authenticate(&token)
        .await
        .map_err(tool_error)?;
    tracing::info!(
        request_id = request_id(parts),
        access_key_id = %principal.key_id.as_uuid(),
        user_id = %principal.user_id.as_uuid(),
        "Access Key authenticated"
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
    use std::sync::Arc;

    use async_trait::async_trait;
    use chaos_core::{
        contracts::{
            AccessKeyListItem, AccessKeyRepository, GeneratedAccessKeyMaterial, McpPrincipal,
            StoreListItem, StoreReadRepository,
        },
        identity::AccessKeyAuthentication,
        store::StoreQueries,
    };
    use chaos_domain::{
        identity::{AccessKey, AccessKeyId, UserId},
        store::{StoreId, StoreRole},
    };
    use secrecy::SecretString;

    use super::*;

    struct FixedAccessKeys(McpPrincipal);

    #[async_trait]
    impl AccessKeyRepository for FixedAccessKeys {
        async fn create(
            &self,
            _key: &AccessKey,
            _material: &GeneratedAccessKeyMaterial,
        ) -> Result<(), ApplicationError> {
            unreachable!()
        }

        async fn list(
            &self,
            _user_id: UserId,
            _after: Option<AccessKeyId>,
            _limit: u16,
        ) -> Result<Vec<AccessKeyListItem>, ApplicationError> {
            unreachable!()
        }

        async fn revoke(
            &self,
            _user_id: UserId,
            _key_id: AccessKeyId,
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
    impl StoreReadRepository for FixedMembership {
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
        let key_id = AccessKeyId::new();
        let store_id = StoreId::new();
        let authentication =
            AccessKeyAuthentication::new(Arc::new(FixedAccessKeys(McpPrincipal {
                key_id,
                user_id,
            })));
        let queries = StoreQueries::new(Arc::new(FixedMembership { user_id, store_id }));
        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer access_test_secret")
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();

        let requested_store_id = store_id.as_uuid().to_string();
        let actor = authenticate_mcp(&authentication, &queries, &parts, &requested_store_id)
            .await
            .unwrap();
        assert_eq!(actor.store_id(), store_id);
        assert_eq!(actor.audit_user_id(), Some(user_id));
        let AdminActor::Store(store_actor) = actor else {
            unreachable!()
        };
        assert_eq!(store_actor.access_key_id(), Some(key_id));
    }

    #[tokio::test]
    async fn rejects_a_store_without_current_membership() {
        let user_id = UserId::new();
        let allowed_store_id = StoreId::new();
        let authentication =
            AccessKeyAuthentication::new(Arc::new(FixedAccessKeys(McpPrincipal {
                key_id: AccessKeyId::new(),
                user_id,
            })));
        let queries = StoreQueries::new(Arc::new(FixedMembership {
            user_id,
            store_id: allowed_store_id,
        }));
        let request = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer access_test_secret")
            .header("x-chaos-store-id", StoreId::new().as_uuid().to_string())
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();

        let requested_store_id = StoreId::new().as_uuid().to_string();
        assert!(
            authenticate_mcp(&authentication, &queries, &parts, &requested_store_id)
                .await
                .is_err()
        );
    }
}
