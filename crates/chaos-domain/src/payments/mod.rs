use uuid::Uuid;

use crate::{DomainError, FieldViolation, pricing::Money, sales::OrderId};

macro_rules! payment_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
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

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

payment_id!(PaymentAttemptId);
payment_id!(RefundId);
payment_id!(PaymentProviderAccountId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentProviderAccount {
    id: PaymentProviderAccountId,
    provider: String,
    display_name: String,
    external_account_reference: String,
    enabled: bool,
}

impl PaymentProviderAccount {
    pub fn create(
        provider: impl Into<String>,
        display_name: impl Into<String>,
        external_account_reference: impl Into<String>,
    ) -> Result<Self, DomainError> {
        Self::rehydrate(
            PaymentProviderAccountId::new(),
            provider,
            display_name,
            external_account_reference,
            true,
        )
    }

    pub fn rehydrate(
        id: PaymentProviderAccountId,
        provider: impl Into<String>,
        display_name: impl Into<String>,
        external_account_reference: impl Into<String>,
        enabled: bool,
    ) -> Result<Self, DomainError> {
        let provider = provider.into();
        let display_name = display_name.into();
        let external_account_reference = external_account_reference.into();
        if provider.is_empty()
            || provider.len() > 64
            || !provider
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(validation(
                "provider",
                "must contain 1-64 lowercase letters, digits, or underscores",
            ));
        }
        validate_printable("display_name", &display_name, 120)?;
        validate_printable(
            "external_account_reference",
            &external_account_reference,
            255,
        )?;
        Ok(Self {
            id,
            provider,
            display_name,
            external_account_reference,
            enabled,
        })
    }

    pub const fn id(&self) -> PaymentProviderAccountId {
        self.id
    }
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn external_account_reference(&self) -> &str {
        &self.external_account_reference
    }
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn update_administration(
        &mut self,
        display_name: impl Into<String>,
        enabled: bool,
    ) -> Result<(), DomainError> {
        let display_name = display_name.into();
        validate_printable("display_name", &display_name, 120)?;
        self.display_name = display_name;
        self.enabled = enabled;
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PaymentSecretReference(String);

impl PaymentSecretReference {
    pub fn new(field: &'static str, value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 255
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
            })
        {
            return Err(validation(
                field,
                "must be a 1-255 character secret-manager reference",
            ));
        }
        Ok(Self(value))
    }

    pub fn expose_reference(&self) -> &str {
        &self.0
    }
}

fn validate_printable(field: &'static str, value: &str, maximum: usize) -> Result<(), DomainError> {
    if value.trim().is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        Err(validation(field, "must contain bounded printable text"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentAttemptStatus {
    Pending,
    Authorized,
    Captured,
    Failed,
    Cancelled,
}

impl PaymentAttemptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Authorized => "authorized",
            Self::Captured => "captured",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "authorized" => Some(Self::Authorized),
            "captured" => Some(Self::Captured),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentAttempt {
    id: PaymentAttemptId,
    order_id: OrderId,
    amount: Money,
    status: PaymentAttemptStatus,
    provider_reference: Option<String>,
}

impl PaymentAttempt {
    pub fn create(order_id: OrderId, amount: Money) -> Result<Self, DomainError> {
        if amount.amount_minor() <= 0 {
            return Err(validation("amount", "must be greater than zero"));
        }
        Ok(Self {
            id: PaymentAttemptId::new(),
            order_id,
            amount,
            status: PaymentAttemptStatus::Pending,
            provider_reference: None,
        })
    }

    pub fn rehydrate(
        id: PaymentAttemptId,
        order_id: OrderId,
        amount: Money,
        status: PaymentAttemptStatus,
        provider_reference: Option<String>,
    ) -> Self {
        Self {
            id,
            order_id,
            amount,
            status,
            provider_reference,
        }
    }

    pub const fn id(&self) -> PaymentAttemptId {
        self.id
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn amount(&self) -> &Money {
        &self.amount
    }

    pub const fn status(&self) -> PaymentAttemptStatus {
        self.status
    }

    pub fn provider_reference(&self) -> Option<&str> {
        self.provider_reference.as_deref()
    }

    pub fn authorize(&mut self, provider_reference: String) -> Result<bool, DomainError> {
        self.bind_provider_reference(provider_reference)?;
        self.advance(PaymentAttemptStatus::Authorized)
    }

    pub fn capture(&mut self) -> Result<bool, DomainError> {
        self.advance(PaymentAttemptStatus::Captured)
    }

    pub fn fail(&mut self, provider_reference: Option<String>) -> Result<bool, DomainError> {
        if let Some(reference) = provider_reference {
            self.bind_provider_reference(reference)?;
        }
        self.advance(PaymentAttemptStatus::Failed)
    }

    pub fn cancel(&mut self, provider_reference: Option<String>) -> Result<bool, DomainError> {
        if let Some(reference) = provider_reference {
            self.bind_provider_reference(reference)?;
        }
        self.advance(PaymentAttemptStatus::Cancelled)
    }

    fn bind_provider_reference(&mut self, value: String) -> Result<(), DomainError> {
        if value.trim().is_empty() || value.chars().count() > 255 {
            return Err(validation(
                "provider_reference",
                "must contain 1-255 characters",
            ));
        }
        match &self.provider_reference {
            Some(existing) if existing != &value => Err(validation(
                "provider_reference",
                "is immutable once assigned",
            )),
            Some(_) => Ok(()),
            None => {
                self.provider_reference = Some(value);
                Ok(())
            }
        }
    }

    fn advance(&mut self, target: PaymentAttemptStatus) -> Result<bool, DomainError> {
        if self.status == target {
            return Ok(false);
        }
        let allowed = matches!(
            (self.status, target),
            (
                PaymentAttemptStatus::Pending,
                PaymentAttemptStatus::Authorized
            ) | (PaymentAttemptStatus::Pending, PaymentAttemptStatus::Failed)
                | (
                    PaymentAttemptStatus::Pending,
                    PaymentAttemptStatus::Cancelled
                )
                | (
                    PaymentAttemptStatus::Authorized,
                    PaymentAttemptStatus::Captured
                )
                | (
                    PaymentAttemptStatus::Authorized,
                    PaymentAttemptStatus::Failed
                )
                | (
                    PaymentAttemptStatus::Authorized,
                    PaymentAttemptStatus::Cancelled
                )
        );
        if !allowed {
            return Err(invalid_transition(self.status.as_str(), target.as_str()));
        }
        self.status = target;
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundStatus {
    Pending,
    Succeeded,
    Failed,
}

impl RefundStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refund {
    id: RefundId,
    payment_attempt_id: PaymentAttemptId,
    amount: Money,
    status: RefundStatus,
    provider_reference: Option<String>,
}

impl Refund {
    pub fn create(
        payment_attempt: &PaymentAttempt,
        amount: Money,
        already_refunded_amount_minor: i64,
    ) -> Result<Self, DomainError> {
        if payment_attempt.status != PaymentAttemptStatus::Captured {
            return Err(validation("payment_attempt", "must be captured"));
        }
        if amount.currency() != payment_attempt.amount.currency() {
            return Err(validation("currency", "must match the Payment Attempt"));
        }
        if amount.amount_minor() <= 0
            || already_refunded_amount_minor < 0
            || amount
                .amount_minor()
                .checked_add(already_refunded_amount_minor)
                .is_none_or(|total| total > payment_attempt.amount.amount_minor())
        {
            return Err(validation(
                "amount",
                "must fit within the remaining captured amount",
            ));
        }
        Ok(Self {
            id: RefundId::new(),
            payment_attempt_id: payment_attempt.id,
            amount,
            status: RefundStatus::Pending,
            provider_reference: None,
        })
    }

    pub fn rehydrate(
        id: RefundId,
        payment_attempt_id: PaymentAttemptId,
        amount: Money,
        status: RefundStatus,
        provider_reference: Option<String>,
    ) -> Self {
        Self {
            id,
            payment_attempt_id,
            amount,
            status,
            provider_reference,
        }
    }

    pub const fn id(&self) -> RefundId {
        self.id
    }

    pub const fn payment_attempt_id(&self) -> PaymentAttemptId {
        self.payment_attempt_id
    }

    pub const fn amount(&self) -> &Money {
        &self.amount
    }

    pub const fn status(&self) -> RefundStatus {
        self.status
    }

    pub fn provider_reference(&self) -> Option<&str> {
        self.provider_reference.as_deref()
    }

    pub fn succeed(&mut self, provider_reference: String) -> Result<bool, DomainError> {
        self.finish(RefundStatus::Succeeded, provider_reference)
    }

    pub fn fail(&mut self, provider_reference: String) -> Result<bool, DomainError> {
        self.finish(RefundStatus::Failed, provider_reference)
    }

    fn finish(
        &mut self,
        target: RefundStatus,
        provider_reference: String,
    ) -> Result<bool, DomainError> {
        if self.status == target && self.provider_reference.as_deref() == Some(&provider_reference)
        {
            return Ok(false);
        }
        if self.status != RefundStatus::Pending {
            return Err(invalid_transition(self.status.as_str(), target.as_str()));
        }
        if provider_reference.trim().is_empty() || provider_reference.chars().count() > 255 {
            return Err(validation(
                "provider_reference",
                "must contain 1-255 characters",
            ));
        }
        self.provider_reference = Some(provider_reference);
        self.status = target;
        Ok(true)
    }
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

fn invalid_transition(from: &str, to: &str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field: "status",
        reason: format!("cannot transition from {from} to {to}"),
    }])
}

#[cfg(test)]
mod tests {
    use crate::{CurrencyCode, pricing::Money};

    use super::*;

    #[test]
    fn payment_attempt_rejects_out_of_order_and_changed_provider_events() {
        let usd = CurrencyCode::parse("USD").unwrap();
        let mut attempt = PaymentAttempt::create(OrderId::new(), Money::new(2_500, usd)).unwrap();

        assert!(attempt.capture().is_err());
        assert!(attempt.authorize("provider-payment-1".into()).unwrap());
        assert!(!attempt.authorize("provider-payment-1".into()).unwrap());
        assert!(attempt.authorize("provider-payment-2".into()).is_err());
        assert!(attempt.capture().unwrap());
        assert!(attempt.fail(None).is_err());

        let mut cancelled = PaymentAttempt::create(OrderId::new(), Money::new(2_500, usd)).unwrap();
        assert!(
            cancelled
                .cancel(Some("provider-cancelled-1".into()))
                .unwrap()
        );
        assert_eq!(cancelled.provider_reference(), Some("provider-cancelled-1"));
        assert!(
            cancelled
                .cancel(Some("provider-cancelled-2".into()))
                .is_err()
        );
    }

    #[test]
    fn refunds_are_currency_safe_and_cannot_exceed_capture() {
        let usd = CurrencyCode::parse("USD").unwrap();
        let mut attempt = PaymentAttempt::create(OrderId::new(), Money::new(2_500, usd)).unwrap();
        attempt.authorize("payment".into()).unwrap();
        attempt.capture().unwrap();

        assert!(Refund::create(&attempt, Money::new(1_500, usd), 1_001).is_err());
        let mut refund = Refund::create(&attempt, Money::new(1_500, usd), 1_000).unwrap();
        assert!(refund.succeed("refund".into()).unwrap());
        assert!(!refund.succeed("refund".into()).unwrap());
        assert!(refund.fail("other".into()).is_err());
    }

    #[test]
    fn provider_accounts_validate_canonical_names_and_opaque_secret_references() {
        assert!(PaymentProviderAccount::create("stripe", "Stripe", "acct_123").is_ok());
        assert!(PaymentProviderAccount::create("Stripe", "Stripe", "acct_123").is_err());
        assert!(
            PaymentSecretReference::new("credential_secret_reference", "vault://stripe/live")
                .is_ok()
        );
        assert!(
            PaymentSecretReference::new("credential_secret_reference", "secret with spaces")
                .is_err()
        );
    }
}
