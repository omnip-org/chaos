use uuid::Uuid;

use crate::{DomainError, FieldViolation, MAX_SECRET_REFERENCE_LEN};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StripeAccountId(Uuid);

impl StripeAccountId {
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

impl Default for StripeAccountId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StripeAccount {
    id: StripeAccountId,
    display_name: String,
}

impl StripeAccount {
    pub fn create(display_name: impl Into<String>) -> Result<Self, DomainError> {
        Self::rehydrate(StripeAccountId::new(), display_name)
    }

    pub fn rehydrate(
        id: StripeAccountId,
        display_name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let display_name = display_name.into();
        validate_printable(&display_name, 120)?;
        Ok(Self { id, display_name })
    }

    pub const fn id(&self) -> StripeAccountId {
        self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn update_administration(
        &mut self,
        display_name: impl Into<String>,
    ) -> Result<(), DomainError> {
        let display_name = display_name.into();
        validate_printable(&display_name, 120)?;
        self.display_name = display_name;
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PaymentSecretReference(String);

impl PaymentSecretReference {
    pub fn new(field: &'static str, value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_SECRET_REFERENCE_LEN
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
            })
        {
            return Err(DomainError::Validation(vec![FieldViolation {
                field,
                reason: "must be a 1-32768 character secret-manager reference".into(),
            }]));
        }
        Ok(Self(value))
    }

    pub fn expose_reference(&self) -> &str {
        &self.0
    }
}

fn validate_printable(value: &str, maximum: usize) -> Result<(), DomainError> {
    if value.trim().is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        Err(DomainError::Validation(vec![FieldViolation {
            field: "display_name",
            reason: "must contain bounded printable text".into(),
        }]))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PaymentSecretReference, StripeAccount};

    #[test]
    fn stripe_accounts_validate_names_and_opaque_secret_references() {
        assert!(StripeAccount::create("Stripe").is_ok());
        assert!(
            PaymentSecretReference::new("credential_secret_reference", "enc://c3RyaXBlLWxpdmU")
                .is_ok()
        );
        assert!(
            PaymentSecretReference::new("credential_secret_reference", "secret with spaces")
                .is_err()
        );
    }
}
