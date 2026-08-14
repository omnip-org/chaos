use std::str::FromStr;

use email_address::EmailAddress;
use uuid::Uuid;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UserId(Uuid);

impl UserId {
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

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Email(String);

impl Email {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let normalized = value.into().trim().to_lowercase();
        if normalized.len() > 320 || EmailAddress::from_str(&normalized).is_err() {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "email",
                reason: "must be a valid email address with at most 320 characters".into(),
            }]));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserStatus {
    Active,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    id: UserId,
    email: Email,
    status: UserStatus,
}

impl User {
    pub fn register(email: Email) -> Self {
        Self {
            id: UserId::new(),
            email,
            status: UserStatus::Active,
        }
    }

    pub const fn id(&self) -> UserId {
        self.id
    }

    pub fn email(&self) -> &Email {
        &self.email
    }

    pub const fn status(&self) -> UserStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_email_for_identity_lookup() {
        let email = Email::parse("  Owner@Example.COM ").unwrap();
        assert_eq!(email.as_str(), "owner@example.com");
    }

    #[test]
    fn rejects_invalid_email() {
        assert!(Email::parse("not-an-email").is_err());
    }
}
