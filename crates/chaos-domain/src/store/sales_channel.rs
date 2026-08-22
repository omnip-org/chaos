use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::StoreId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SalesChannelId(Uuid);

impl SalesChannelId {
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

impl Default for SalesChannelId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SalesChannelCode(String);

impl SalesChannelCode {
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
pub enum SalesChannelStatus {
    Active,
    Archived,
}

impl SalesChannelStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SalesChannel {
    id: SalesChannelId,
    store_id: StoreId,
    code: SalesChannelCode,
    name: String,
    status: SalesChannelStatus,
    is_default: bool,
}

impl SalesChannel {
    pub fn create(
        store_id: StoreId,
        code: SalesChannelCode,
        name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-120 characters".into(),
            }]));
        }
        Ok(Self {
            id: SalesChannelId::new(),
            store_id,
            code,
            name,
            status: SalesChannelStatus::Active,
            is_default: false,
        })
    }

    pub fn default_web(store_id: StoreId) -> Self {
        Self {
            id: SalesChannelId::new(),
            store_id,
            code: SalesChannelCode("web".into()),
            name: "Online Store".into(),
            status: SalesChannelStatus::Active,
            is_default: true,
        }
    }

    pub const fn id(&self) -> SalesChannelId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub fn code(&self) -> &SalesChannelCode {
        &self.code
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn status(&self) -> SalesChannelStatus {
        self.status
    }

    pub const fn is_default(&self) -> bool {
        self.is_default
    }

    pub fn validate_archival(is_default: bool) -> Result<(), DomainError> {
        if is_default {
            Err(DomainError::Validation(vec![FieldViolation {
                field: "sales_channel_id",
                reason: "the default Sales Channel cannot be archived".into(),
            }]))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_channel_is_an_active_web_surface() {
        let channel = SalesChannel::default_web(StoreId::new());

        assert_eq!(channel.code().as_str(), "web");
        assert_eq!(channel.status(), SalesChannelStatus::Active);
        assert!(channel.is_default());
    }

    #[test]
    fn custom_channel_validates_content_and_default_archival() {
        let channel = SalesChannel::create(
            StoreId::new(),
            SalesChannelCode::parse("mobile-app").unwrap(),
            "Mobile App",
        )
        .unwrap();
        assert!(!channel.is_default());
        assert!(SalesChannel::validate_archival(false).is_ok());
        assert!(SalesChannel::validate_archival(true).is_err());
    }
}
