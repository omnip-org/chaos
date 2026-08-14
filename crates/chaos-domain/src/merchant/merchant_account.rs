use uuid::Uuid;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MerchantAccountId(Uuid);

impl MerchantAccountId {
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

impl Default for MerchantAccountId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MerchantAccountSlug(String);

impl MerchantAccountSlug {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (3..=63).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');

        if !valid {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "slug",
                reason: "must be 3-63 lowercase ASCII letters, digits, or hyphens, and start and end with a letter or digit".into(),
            }]));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MerchantAccountStatus {
    Active,
    Suspended,
    Closed,
}

impl MerchantAccountStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantAccount {
    id: MerchantAccountId,
    slug: MerchantAccountSlug,
    display_name: String,
    status: MerchantAccountStatus,
}

impl MerchantAccount {
    pub fn create(
        slug: MerchantAccountSlug,
        display_name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() || display_name.chars().count() > 120 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "display_name",
                reason: "must contain 1-120 characters".into(),
            }]));
        }

        Ok(Self {
            id: MerchantAccountId::new(),
            slug,
            display_name,
            status: MerchantAccountStatus::Active,
        })
    }

    pub const fn id(&self) -> MerchantAccountId {
        self.id
    }

    pub fn slug(&self) -> &MerchantAccountSlug {
        &self.slug
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn status(&self) -> MerchantAccountStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_canonical_slug() {
        assert_eq!(
            MerchantAccountSlug::parse("acme-sg").unwrap().as_str(),
            "acme-sg"
        );
    }

    #[test]
    fn rejects_a_slug_that_would_need_implicit_normalization() {
        assert!(MerchantAccountSlug::parse("Acme SG").is_err());
    }

    #[test]
    fn merchant_account_starts_active() {
        let account =
            MerchantAccount::create(MerchantAccountSlug::parse("acme").unwrap(), "ACME").unwrap();
        assert_eq!(account.status(), MerchantAccountStatus::Active);
    }
}
