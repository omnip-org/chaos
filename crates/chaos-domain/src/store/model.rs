use uuid::Uuid;

use crate::{CurrencyCode, DomainError, FieldViolation, RegionCode};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreId(Uuid);

impl StoreId {
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

impl Default for StoreId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreStatus {
    Active,
    Inactive,
}

impl StoreStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Store {
    id: StoreId,
    name: String,
    region: RegionCode,
    currency: CurrencyCode,
    meta: Option<serde_json::Value>,
    status: StoreStatus,
}

impl Store {
    pub fn create(
        name: impl Into<String>,
        region: RegionCode,
        currency: CurrencyCode,
        meta: Option<serde_json::Value>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-120 characters".into(),
            }]));
        }
        Ok(Self {
            id: StoreId::new(),
            name,
            region,
            currency,
            meta,
            status: StoreStatus::Active,
        })
    }

    pub const fn id(&self) -> StoreId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }

    pub const fn region(&self) -> RegionCode {
        self.region
    }

    pub const fn status(&self) -> StoreStatus {
        self.status
    }

    pub fn meta(&self) -> Option<&serde_json::Value> {
        self.meta.as_ref()
    }

    pub fn validate_activation(active_channel_exists: bool) -> Result<(), DomainError> {
        let mut violations = Vec::new();
        if !active_channel_exists {
            violations.push(FieldViolation {
                field: "channels",
                reason: "must contain an active channel before Store activation".into(),
            });
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(DomainError::Validation(violations))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_store_starts_active() {
        let store = Store::create("Main Store", RegionCode::US, CurrencyCode::USD, None).unwrap();
        assert_eq!(store.status(), StoreStatus::Active);
        assert_eq!(store.region(), RegionCode::US);
        assert_eq!(store.currency(), CurrencyCode::USD);
    }

    #[test]
    fn activation_requires_channel_readiness() {
        assert!(Store::validate_activation(true).is_ok());
        let error = Store::validate_activation(false).unwrap_err();
        assert!(matches!(error, DomainError::Validation(violations) if violations.len() == 1));
    }
}
