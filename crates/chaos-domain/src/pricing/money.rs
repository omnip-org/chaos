use crate::{CurrencyCode, DomainError, FieldViolation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Money {
    amount_minor: i64,
    currency: CurrencyCode,
}

impl Money {
    pub const fn new(amount_minor: i64, currency: CurrencyCode) -> Self {
        Self {
            amount_minor,
            currency,
        }
    }

    pub const fn zero(currency: CurrencyCode) -> Self {
        Self::new(0, currency)
    }

    pub const fn amount_minor(&self) -> i64 {
        self.amount_minor
    }

    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, DomainError> {
        self.require_same_currency(other)?;
        let amount_minor = self
            .amount_minor
            .checked_add(other.amount_minor)
            .ok_or_else(arithmetic_overflow)?;
        Ok(Self::new(amount_minor, self.currency))
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, DomainError> {
        self.require_same_currency(other)?;
        let amount_minor = self
            .amount_minor
            .checked_sub(other.amount_minor)
            .ok_or_else(arithmetic_overflow)?;
        Ok(Self::new(amount_minor, self.currency))
    }

    pub fn checked_mul(&self, quantity: u64) -> Result<Self, DomainError> {
        let amount = i128::from(self.amount_minor)
            .checked_mul(i128::from(quantity))
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(arithmetic_overflow)?;
        Ok(Self::new(amount, self.currency))
    }

    pub fn allocate(&self, ratios: &[u64]) -> Result<Vec<Self>, DomainError> {
        if ratios.is_empty() || ratios.iter().all(|ratio| *ratio == 0) {
            return Err(validation(
                "ratios",
                "must contain at least one positive allocation ratio",
            ));
        }
        let total_ratio = ratios.iter().try_fold(0_u128, |total, ratio| {
            total
                .checked_add(u128::from(*ratio))
                .ok_or_else(arithmetic_overflow)
        })?;
        let total_ratio = i128::try_from(total_ratio).map_err(|_| arithmetic_overflow())?;
        let mut allocated = ratios
            .iter()
            .map(|ratio| {
                i128::from(self.amount_minor)
                    .checked_mul(i128::from(*ratio))
                    .map(|weighted| weighted / total_ratio)
                    .and_then(|amount| i64::try_from(amount).ok())
                    .ok_or_else(arithmetic_overflow)
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        let allocated_total = allocated.iter().try_fold(0_i64, |total, amount| {
            total.checked_add(*amount).ok_or_else(arithmetic_overflow)
        })?;
        let remainder = self
            .amount_minor
            .checked_sub(allocated_total)
            .ok_or_else(arithmetic_overflow)?;
        let adjustment = remainder.signum();
        let eligible_indices = ratios
            .iter()
            .enumerate()
            .filter_map(|(index, ratio)| (*ratio > 0).then_some(index))
            .collect::<Vec<_>>();
        for index in 0..remainder.unsigned_abs() as usize {
            let allocation_index = eligible_indices[index % eligible_indices.len()];
            allocated[allocation_index] = allocated[allocation_index]
                .checked_add(adjustment)
                .ok_or_else(arithmetic_overflow)?;
        }
        Ok(allocated
            .into_iter()
            .map(|amount| Self::new(amount, self.currency))
            .collect())
    }

    fn require_same_currency(&self, other: &Self) -> Result<(), DomainError> {
        if self.currency == other.currency {
            return Ok(());
        }
        Err(validation(
            "currency",
            "money operations require matching currencies",
        ))
    }
}

fn arithmetic_overflow() -> DomainError {
    validation("amount_minor", "money arithmetic overflowed")
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
    fn adds_and_subtracts_only_matching_currencies() {
        let ten = Money::new(1_000, CurrencyCode::USD);
        let five = Money::new(500, CurrencyCode::USD);

        assert_eq!(ten.checked_add(&five).unwrap().amount_minor(), 1_500);
        assert_eq!(ten.checked_sub(&five).unwrap().amount_minor(), 500);
        assert!(
            ten.checked_add(&Money::new(500, CurrencyCode::parse("EUR").unwrap()))
                .is_err()
        );
    }

    #[test]
    fn multiplication_is_checked() {
        assert_eq!(
            Money::new(125, CurrencyCode::USD)
                .checked_mul(4)
                .unwrap()
                .amount_minor(),
            500
        );
        assert!(
            Money::new(i64::MAX, CurrencyCode::USD)
                .checked_mul(2)
                .is_err()
        );
    }

    #[test]
    fn allocation_preserves_positive_and_negative_totals_deterministically() {
        let positive = Money::new(10, CurrencyCode::USD)
            .allocate(&[1, 1, 1])
            .unwrap();
        assert_eq!(
            positive.iter().map(Money::amount_minor).collect::<Vec<_>>(),
            vec![4, 3, 3]
        );

        let negative = Money::new(-10, CurrencyCode::USD)
            .allocate(&[1, 1, 1])
            .unwrap();
        assert_eq!(
            negative.iter().map(Money::amount_minor).collect::<Vec<_>>(),
            vec![-4, -3, -3]
        );
    }

    #[test]
    fn allocation_supports_zero_weight_and_rejects_zero_total() {
        let allocation = Money::new(11, CurrencyCode::USD)
            .allocate(&[0, 1, 1])
            .unwrap();
        assert_eq!(
            allocation
                .iter()
                .map(Money::amount_minor)
                .collect::<Vec<_>>(),
            vec![0, 6, 5]
        );
        assert!(Money::new(1, CurrencyCode::USD).allocate(&[]).is_err());
        assert!(Money::new(1, CurrencyCode::USD).allocate(&[0, 0]).is_err());
    }

    #[test]
    fn all_arithmetic_edges_are_checked() {
        assert!(
            Money::new(i64::MAX, CurrencyCode::USD)
                .checked_add(&Money::new(1, CurrencyCode::USD))
                .is_err()
        );
        assert!(
            Money::new(i64::MIN, CurrencyCode::USD)
                .checked_sub(&Money::new(1, CurrencyCode::USD))
                .is_err()
        );
    }
}
