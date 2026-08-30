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

payment_id!(RefundId);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentAttemptStatus {
    Pending,
    Captured,
    Failed,
}

impl PaymentAttemptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Captured => "captured",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "captured" => Some(Self::Captured),
            "failed" => Some(Self::Failed),
            _ => None,
        }
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
    order_id: OrderId,
    amount: Money,
    status: RefundStatus,
    provider_reference: Option<String>,
}

impl Refund {
    pub fn create(
        order_id: OrderId,
        payment_status: PaymentAttemptStatus,
        captured_amount: Money,
        amount: Money,
        already_refunded_amount_minor: i64,
    ) -> Result<Self, DomainError> {
        if payment_status != PaymentAttemptStatus::Captured {
            return Err(validation("order", "payment must be captured"));
        }
        if amount.currency() != captured_amount.currency() {
            return Err(validation("currency", "must match the captured payment"));
        }
        if amount.amount_minor() <= 0
            || already_refunded_amount_minor < 0
            || amount
                .amount_minor()
                .checked_add(already_refunded_amount_minor)
                .is_none_or(|total| total > captured_amount.amount_minor())
        {
            return Err(validation(
                "amount",
                "must fit within the remaining captured amount",
            ));
        }
        Ok(Self {
            id: RefundId::new(),
            order_id,
            amount,
            status: RefundStatus::Pending,
            provider_reference: None,
        })
    }

    pub fn rehydrate(
        id: RefundId,
        order_id: OrderId,
        amount: Money,
        status: RefundStatus,
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

    pub const fn id(&self) -> RefundId {
        self.id
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
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
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use crate::{CurrencyCode, pricing::Money};

    use super::*;

    #[test]
    fn refunds_are_currency_safe_and_cannot_exceed_capture() {
        let usd = CurrencyCode::parse("USD").unwrap();
        let order_id = OrderId::new();
        let captured = Money::new(2_500, usd);

        assert!(
            Refund::create(
                order_id,
                PaymentAttemptStatus::Pending,
                captured.clone(),
                Money::new(1_500, usd),
                0,
            )
            .is_err()
        );
        assert!(
            Refund::create(
                order_id,
                PaymentAttemptStatus::Captured,
                captured.clone(),
                Money::new(1_500, usd),
                1_001,
            )
            .is_err()
        );
        let refund = Refund::create(
            order_id,
            PaymentAttemptStatus::Captured,
            captured,
            Money::new(1_500, usd),
            1_000,
        )
        .unwrap();
        assert_eq!(refund.status(), RefundStatus::Pending);
    }
}
