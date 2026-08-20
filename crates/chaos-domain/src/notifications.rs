use uuid::Uuid;

use crate::{DomainError, FieldViolation, MAX_SECRET_REFERENCE_LEN};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NotificationProviderAccountId(Uuid);

impl NotificationProviderAccountId {
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

impl Default for NotificationProviderAccountId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationProviderAccount {
    id: NotificationProviderAccountId,
    provider: String,
    display_name: String,
    sender: String,
    enabled: bool,
}

impl NotificationProviderAccount {
    pub fn create(
        provider: impl Into<String>,
        display_name: impl Into<String>,
        sender: impl Into<String>,
        enabled: bool,
    ) -> Result<Self, DomainError> {
        Self::build(
            NotificationProviderAccountId::new(),
            provider,
            display_name,
            sender,
            enabled,
        )
    }

    pub fn rehydrate(
        id: NotificationProviderAccountId,
        provider: impl Into<String>,
        display_name: impl Into<String>,
        sender: impl Into<String>,
        enabled: bool,
    ) -> Result<Self, DomainError> {
        Self::build(id, provider, display_name, sender, enabled)
    }

    fn build(
        id: NotificationProviderAccountId,
        provider: impl Into<String>,
        display_name: impl Into<String>,
        sender: impl Into<String>,
        enabled: bool,
    ) -> Result<Self, DomainError> {
        let provider = provider.into();
        if provider != "resend" {
            return Err(validation("provider", "must be resend"));
        }
        let display_name = display_name.into();
        validate_text("display_name", &display_name, 120)?;
        let sender = sender.into();
        if sender.len() < 3
            || sender.len() > 320
            || !sender.contains('@')
            || sender.chars().any(char::is_control)
        {
            return Err(validation("sender", "must be a valid bounded email sender"));
        }
        Ok(Self {
            id,
            provider,
            display_name,
            sender,
            enabled,
        })
    }

    pub const fn id(&self) -> NotificationProviderAccountId {
        self.id
    }
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn sender(&self) -> &str {
        &self.sender
    }
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationSecretReference(String);

impl NotificationSecretReference {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.len() < 8
            || value.len() > MAX_SECRET_REFERENCE_LEN
            || value.chars().any(char::is_whitespace)
            || !value.contains("://")
        {
            return Err(validation(
                "secret_reference",
                "must be an opaque secret-manager reference",
            ));
        }
        Ok(Self(value))
    }

    pub fn expose_reference(&self) -> &str {
        &self.0
    }
}

fn validate_text(field: &'static str, value: &str, max: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(validation(
            field,
            &format!("must contain 1-{max} control-free characters"),
        ));
    }
    Ok(())
}

fn validation(field: &'static str, reason: &str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_provider_is_resend_and_requires_a_sender() {
        assert!(
            NotificationProviderAccount::create(
                "resend",
                "Primary",
                "Shop <orders@example.com>",
                true
            )
            .is_ok()
        );
        assert!(
            NotificationProviderAccount::create("smtp", "Primary", "orders@example.com", true)
                .is_err()
        );
        assert!(NotificationProviderAccount::create("resend", "Primary", "invalid", true).is_err());
    }
}
