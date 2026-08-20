use std::collections::HashSet;

use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::StoreId;

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
}

impl ApiKeyClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Publishable => "publishable",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "publishable" => Some(Self::Publishable),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApiKeyScope {
    AnalyticsWrite,
    CatalogRead,
    CartsWrite,
    CheckoutWrite,
    OrdersRead,
    ReviewsWrite,
}

impl ApiKeyScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnalyticsWrite => "analytics:write",
            Self::CatalogRead => "catalog:read",
            Self::CartsWrite => "carts:write",
            Self::CheckoutWrite => "checkout:write",
            Self::OrdersRead => "orders:read",
            Self::ReviewsWrite => "reviews:write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "analytics:write" => Some(Self::AnalyticsWrite),
            "catalog:read" => Some(Self::CatalogRead),
            "carts:write" => Some(Self::CartsWrite),
            "checkout:write" => Some(Self::CheckoutWrite),
            "orders:read" => Some(Self::OrdersRead),
            "reviews:write" => Some(Self::ReviewsWrite),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiKey {
    id: ApiKeyId,
    store_id: StoreId,
    name: String,
    class: ApiKeyClass,
    scopes: Vec<ApiKeyScope>,
}

impl ApiKey {
    pub fn issue(
        store_id: StoreId,
        name: impl Into<String>,
        class: ApiKeyClass,
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
        Ok(Self {
            id: ApiKeyId::new(),
            store_id,
            name,
            class,
            scopes,
        })
    }

    pub const fn id(&self) -> ApiKeyId {
        self.id
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

    pub fn scopes(&self) -> &[ApiKeyScope] {
        &self.scopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishable_key_accepts_storefront_scopes() {
        let key = ApiKey::issue(
            StoreId::new(),
            "Browser",
            ApiKeyClass::Publishable,
            vec![
                ApiKeyScope::CatalogRead,
                ApiKeyScope::CartsWrite,
                ApiKeyScope::CheckoutWrite,
                ApiKeyScope::AnalyticsWrite,
                ApiKeyScope::OrdersRead,
                ApiKeyScope::ReviewsWrite,
            ],
        )
        .unwrap();
        assert!(key.scopes().contains(&ApiKeyScope::CheckoutWrite));
        assert!(key.scopes().contains(&ApiKeyScope::OrdersRead));
        assert!(key.scopes().contains(&ApiKeyScope::ReviewsWrite));
    }

    #[test]
    fn key_rejects_duplicate_scopes() {
        let result = ApiKey::issue(
            StoreId::new(),
            "Duplicate",
            ApiKeyClass::Publishable,
            vec![ApiKeyScope::CatalogRead, ApiKeyScope::CatalogRead],
        );

        assert!(matches!(result, Err(DomainError::Validation(_))));
    }
}
