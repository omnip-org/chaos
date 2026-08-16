use std::net::IpAddr;

use uuid::Uuid;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreDomainId(Uuid);

impl StoreDomainId {
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

impl Default for StoreDomainId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreHostname(String);

impl StoreHostname {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let labels = value.split('.').collect::<Vec<_>>();
        let valid_labels = labels.len() >= 2
            && labels.iter().all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    && label
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
            });
        let reserved = labels.last().is_some_and(|label| {
            matches!(
                *label,
                "example" | "invalid" | "local" | "localhost" | "onion" | "test"
            )
        });
        if value.is_empty()
            || value.len() > 253
            || !value.is_ascii()
            || value.ends_with('.')
            || value.parse::<IpAddr>().is_ok()
            || !valid_labels
            || reserved
        {
            return Err(validation(
                "hostname",
                "must be a canonical lowercase public DNS hostname",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn verification_record_name(&self) -> String {
        format!("_chaos-verification.{}.", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreDomainStatus {
    Pending,
    Verified,
    Archived,
}

impl StoreDomainStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "verified" => Some(Self::Verified),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_requires_canonical_public_dns_form() {
        assert_eq!(
            StoreHostname::parse("shop.example-commerce.com")
                .unwrap()
                .as_str(),
            "shop.example-commerce.com"
        );
        for invalid in [
            "SHOP.EXAMPLE.COM",
            "shop.example.com.",
            "localhost",
            "127.0.0.1",
            "shop.local",
            "shop.example",
            "-shop.example.com",
            "shop_.example.com",
        ] {
            assert!(StoreHostname::parse(invalid).is_err(), "{invalid}");
        }
    }
}
