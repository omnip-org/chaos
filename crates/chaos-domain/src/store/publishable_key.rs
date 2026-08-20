use std::collections::HashSet;

use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::StoreId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PublishableKeyId(Uuid);

impl PublishableKeyId {
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

impl Default for PublishableKeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PublishableKeyScope {
    AnalyticsWrite,
    CatalogRead,
    CartsWrite,
    CheckoutWrite,
    OrdersRead,
    ReviewsWrite,
}

impl PublishableKeyScope {
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
pub struct PublishableKey {
    id: PublishableKeyId,
    store_id: StoreId,
    name: String,
    scopes: Vec<PublishableKeyScope>,
}

impl PublishableKey {
    pub fn issue(
        store_id: StoreId,
        name: impl Into<String>,
        scopes: Vec<PublishableKeyScope>,
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
            id: PublishableKeyId::new(),
            store_id,
            name,
            scopes,
        })
    }

    pub const fn id(&self) -> PublishableKeyId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn scopes(&self) -> &[PublishableKeyScope] {
        &self.scopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishable_key_accepts_storefront_scopes() {
        let key = PublishableKey::issue(
            StoreId::new(),
            "Browser",
            vec![
                PublishableKeyScope::CatalogRead,
                PublishableKeyScope::CartsWrite,
                PublishableKeyScope::CheckoutWrite,
                PublishableKeyScope::AnalyticsWrite,
                PublishableKeyScope::OrdersRead,
                PublishableKeyScope::ReviewsWrite,
            ],
        )
        .unwrap();
        assert!(key.scopes().contains(&PublishableKeyScope::CheckoutWrite));
        assert!(key.scopes().contains(&PublishableKeyScope::OrdersRead));
        assert!(key.scopes().contains(&PublishableKeyScope::ReviewsWrite));
    }

    #[test]
    fn key_rejects_duplicate_scopes() {
        let result = PublishableKey::issue(
            StoreId::new(),
            "Duplicate",
            vec![
                PublishableKeyScope::CatalogRead,
                PublishableKeyScope::CatalogRead,
            ],
        );

        assert!(matches!(result, Err(DomainError::Validation(_))));
    }
}
