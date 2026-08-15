use uuid::Uuid;

use crate::{CurrencyCode, DomainError, FieldViolation, RegionCode};

use super::MerchantAccountId;

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoreCode(String);

impl StoreCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (2..=32).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        if !valid {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "code",
                reason: "must be 2-32 lowercase ASCII letters, digits, or hyphens".into(),
            }]));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreStatus {
    Draft,
    Active,
    Archived,
}

impl StoreStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Store {
    id: StoreId,
    merchant_account_id: MerchantAccountId,
    code: StoreCode,
    name: String,
    default_region: RegionCode,
    default_currency: CurrencyCode,
    status: StoreStatus,
}

impl Store {
    pub fn create(
        merchant_account_id: MerchantAccountId,
        code: StoreCode,
        name: impl Into<String>,
        default_region: RegionCode,
        default_currency: CurrencyCode,
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
            merchant_account_id,
            code,
            name,
            default_region,
            default_currency,
            status: StoreStatus::Draft,
        })
    }

    pub const fn id(&self) -> StoreId {
        self.id
    }

    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }

    pub fn code(&self) -> &StoreCode {
        &self.code
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn default_currency(&self) -> CurrencyCode {
        self.default_currency
    }

    pub const fn default_region(&self) -> RegionCode {
        self.default_region
    }

    pub const fn status(&self) -> StoreStatus {
        self.status
    }

    pub fn validate_activation(
        default_currency_enabled: bool,
        active_default_channel_exists: bool,
    ) -> Result<(), DomainError> {
        let mut violations = Vec::new();
        if !default_currency_enabled {
            violations.push(FieldViolation {
                field: "default_currency",
                reason: "must be enabled before Store activation".into(),
            });
        }
        if !active_default_channel_exists {
            violations.push(FieldViolation {
                field: "sales_channels",
                reason: "must contain an active default channel before Store activation".into(),
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
    fn new_store_starts_as_draft() {
        let store = Store::create(
            MerchantAccountId::new(),
            StoreCode::parse("main-store").unwrap(),
            "Main Store",
            RegionCode::US,
            CurrencyCode::USD,
        )
        .unwrap();
        assert_eq!(store.status(), StoreStatus::Draft);
        assert_eq!(store.default_region(), RegionCode::US);
        assert_eq!(store.default_currency(), CurrencyCode::USD);
    }

    #[test]
    fn activation_requires_currency_and_default_channel_readiness() {
        assert!(Store::validate_activation(true, true).is_ok());
        let error = Store::validate_activation(false, false).unwrap_err();
        assert!(matches!(error, DomainError::Validation(violations) if violations.len() == 2));
    }
}
