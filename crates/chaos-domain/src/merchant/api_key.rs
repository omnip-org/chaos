use std::collections::HashSet;

use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::{MerchantAccountId, StoreId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ApiKeyId(Uuid);

impl ApiKeyId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ApiKeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiKeyClass {
    Publishable,
    Secret,
}

impl ApiKeyClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Publishable => "publishable",
            Self::Secret => "secret",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "publishable" => Some(Self::Publishable),
            "secret" => Some(Self::Secret),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiKeyMode {
    Test,
    Live,
}

impl ApiKeyMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Live => "live",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "test" => Some(Self::Test),
            "live" => Some(Self::Live),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApiKeyScope {
    CatalogRead,
    CartsWrite,
    CheckoutWrite,
    OrdersRead,
    CustomersWrite,
    McpTools,
}

impl ApiKeyScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatalogRead => "catalog:read",
            Self::CartsWrite => "carts:write",
            Self::CheckoutWrite => "checkout:write",
            Self::OrdersRead => "orders:read",
            Self::CustomersWrite => "customers:write",
            Self::McpTools => "mcp:tools",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "catalog:read" => Some(Self::CatalogRead),
            "carts:write" => Some(Self::CartsWrite),
            "checkout:write" => Some(Self::CheckoutWrite),
            "orders:read" => Some(Self::OrdersRead),
            "customers:write" => Some(Self::CustomersWrite),
            "mcp:tools" => Some(Self::McpTools),
            _ => None,
        }
    }

    const fn allowed_for_publishable_key(self) -> bool {
        matches!(self, Self::CatalogRead)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiKey {
    id: ApiKeyId,
    merchant_account_id: MerchantAccountId,
    store_id: StoreId,
    name: String,
    class: ApiKeyClass,
    mode: ApiKeyMode,
    scopes: Vec<ApiKeyScope>,
}

impl ApiKey {
    pub fn issue(
        merchant_account_id: MerchantAccountId,
        store_id: StoreId,
        name: impl Into<String>,
        class: ApiKeyClass,
        mode: ApiKeyMode,
        scopes: Vec<ApiKeyScope>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 80 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-80 characters".into(),
            }]));
        }
        if scopes.is_empty() {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "scopes",
                reason: "must contain at least one scope".into(),
            }]));
        }
        if scopes.iter().copied().collect::<HashSet<_>>().len() != scopes.len() {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "scopes",
                reason: "must not contain duplicate scopes".into(),
            }]));
        }
        if class == ApiKeyClass::Publishable
            && scopes
                .iter()
                .any(|scope| !scope.allowed_for_publishable_key())
        {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "scopes",
                reason: "publishable keys may contain only public read scopes".into(),
            }]));
        }

        Ok(Self {
            id: ApiKeyId::new(),
            merchant_account_id,
            store_id,
            name,
            class,
            mode,
            scopes,
        })
    }

    pub const fn id(&self) -> ApiKeyId {
        self.id
    }

    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn class(&self) -> ApiKeyClass {
        self.class
    }

    pub const fn mode(&self) -> ApiKeyMode {
        self.mode
    }

    pub fn scopes(&self) -> &[ApiKeyScope] {
        &self.scopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_key_can_receive_mcp_scope() {
        let key = ApiKey::issue(
            MerchantAccountId::new(),
            StoreId::new(),
            "Production MCP",
            ApiKeyClass::Secret,
            ApiKeyMode::Live,
            vec![ApiKeyScope::McpTools, ApiKeyScope::OrdersRead],
        )
        .unwrap();

        assert_eq!(key.class(), ApiKeyClass::Secret);
        assert!(key.scopes().contains(&ApiKeyScope::McpTools));
    }

    #[test]
    fn publishable_key_rejects_private_scopes() {
        let result = ApiKey::issue(
            MerchantAccountId::new(),
            StoreId::new(),
            "Browser",
            ApiKeyClass::Publishable,
            ApiKeyMode::Live,
            vec![ApiKeyScope::CheckoutWrite],
        );

        assert!(matches!(result, Err(DomainError::Validation(_))));
    }

    #[test]
    fn key_rejects_duplicate_scopes() {
        let result = ApiKey::issue(
            MerchantAccountId::new(),
            StoreId::new(),
            "Duplicate",
            ApiKeyClass::Secret,
            ApiKeyMode::Test,
            vec![ApiKeyScope::CatalogRead, ApiKeyScope::CatalogRead],
        );

        assert!(matches!(result, Err(DomainError::Validation(_))));
    }
}
