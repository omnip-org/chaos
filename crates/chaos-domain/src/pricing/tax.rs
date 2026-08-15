use uuid::Uuid;

use crate::{DomainError, FieldViolation, pricing::Money};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaxRuleId(Uuid);

impl TaxRuleId {
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

impl Default for TaxRuleId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaxRuleStatus {
    Active,
    Archived,
}

impl TaxRuleStatus {
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
pub struct TaxRule {
    id: TaxRuleId,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
    status: TaxRuleStatus,
}

impl TaxRule {
    pub fn create(
        code: impl Into<String>,
        name: impl Into<String>,
        country_code: impl Into<String>,
        rate_basis_points: u32,
    ) -> Result<Self, DomainError> {
        Self::rehydrate(
            TaxRuleId::new(),
            code,
            name,
            country_code,
            rate_basis_points,
            TaxRuleStatus::Active,
        )
    }

    pub fn rehydrate(
        id: TaxRuleId,
        code: impl Into<String>,
        name: impl Into<String>,
        country_code: impl Into<String>,
        rate_basis_points: u32,
        status: TaxRuleStatus,
    ) -> Result<Self, DomainError> {
        let code = code.into();
        let name = name.into();
        let country_code = country_code.into().trim().to_ascii_uppercase();
        if code.is_empty()
            || code.len() > 64
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(validation(
                "code",
                "must contain 1-64 lowercase letters, digits, or hyphens",
            ));
        }
        if name.trim().is_empty()
            || name.chars().count() > 120
            || name.chars().any(char::is_control)
        {
            return Err(validation(
                "name",
                "must contain 1-120 printable characters",
            ));
        }
        if country_code.len() != 2 || !country_code.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(validation(
                "country_code",
                "must be an ISO 3166-1 alpha-2 country code",
            ));
        }
        if rate_basis_points > 10_000 {
            return Err(validation(
                "rate_basis_points",
                "must be between 0 and 10000",
            ));
        }
        Ok(Self {
            id,
            code,
            name,
            country_code,
            rate_basis_points,
            status,
        })
    }

    pub const fn id(&self) -> TaxRuleId {
        self.id
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn country_code(&self) -> &str {
        &self.country_code
    }
    pub const fn rate_basis_points(&self) -> u32 {
        self.rate_basis_points
    }
    pub const fn status(&self) -> TaxRuleStatus {
        self.status
    }

    pub fn calculate_and_allocate(
        &self,
        taxable_amounts: &[Money],
        tax_inclusive: bool,
    ) -> Result<Vec<Money>, DomainError> {
        if self.status != TaxRuleStatus::Active {
            return Err(validation(
                "tax_rule_id",
                "must reference an active Tax Rule",
            ));
        }
        let Some(first) = taxable_amounts.first() else {
            return Err(validation("taxable_amounts", "must not be empty"));
        };
        if taxable_amounts
            .iter()
            .any(|amount| amount.amount_minor() < 0 || amount.currency() != first.currency())
        {
            return Err(validation(
                "taxable_amounts",
                "must be non-negative and use one currency",
            ));
        }
        let taxable_total = taxable_amounts.iter().try_fold(0_i128, |total, amount| {
            total
                .checked_add(i128::from(amount.amount_minor()))
                .ok_or_else(arithmetic_overflow)
        })?;
        if taxable_total == 0 || self.rate_basis_points == 0 {
            return Ok(taxable_amounts
                .iter()
                .map(|_| Money::zero(first.currency()))
                .collect());
        }
        let rate = i128::from(self.rate_basis_points);
        let denominator = if tax_inclusive {
            10_000_i128 + rate
        } else {
            10_000_i128
        };
        let numerator = taxable_total
            .checked_mul(rate)
            .ok_or_else(arithmetic_overflow)?;
        let rounded = numerator
            .checked_add(denominator / 2)
            .ok_or_else(arithmetic_overflow)?
            / denominator;
        let tax_total = i64::try_from(rounded).map_err(|_| arithmetic_overflow())?;
        let weights = taxable_amounts
            .iter()
            .map(|amount| u64::try_from(amount.amount_minor()).map_err(|_| arithmetic_overflow()))
            .collect::<Result<Vec<_>, _>>()?;
        Money::new(tax_total, first.currency()).allocate(&weights)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaxRuleSnapshot {
    rule_id: TaxRuleId,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
}

impl TaxRuleSnapshot {
    pub fn from_rule(rule: &TaxRule) -> Result<Self, DomainError> {
        if rule.status() != TaxRuleStatus::Active {
            return Err(validation(
                "tax_rule_id",
                "must reference an active Tax Rule",
            ));
        }
        Ok(Self {
            rule_id: rule.id(),
            code: rule.code().into(),
            name: rule.name().into(),
            country_code: rule.country_code().into(),
            rate_basis_points: rule.rate_basis_points(),
        })
    }

    pub fn rehydrate(
        rule_id: TaxRuleId,
        code: impl Into<String>,
        name: impl Into<String>,
        country_code: impl Into<String>,
        rate_basis_points: u32,
    ) -> Result<Self, DomainError> {
        let rule = TaxRule::rehydrate(
            rule_id,
            code,
            name,
            country_code,
            rate_basis_points,
            TaxRuleStatus::Active,
        )?;
        Self::from_rule(&rule)
    }

    pub const fn rule_id(&self) -> TaxRuleId {
        self.rule_id
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn country_code(&self) -> &str {
        &self.country_code
    }
    pub const fn rate_basis_points(&self) -> u32 {
        self.rate_basis_points
    }
}

fn arithmetic_overflow() -> DomainError {
    validation("amount_minor", "tax arithmetic overflowed")
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use crate::CurrencyCode;

    use super::*;

    #[test]
    fn tax_is_rounded_once_then_allocated_deterministically() {
        let rule = TaxRule::create("sales-tax", "Sales tax", "us", 825).unwrap();
        let taxes = rule
            .calculate_and_allocate(
                &[
                    Money::new(100, CurrencyCode::USD),
                    Money::new(100, CurrencyCode::USD),
                ],
                false,
            )
            .unwrap();
        assert_eq!(
            taxes.iter().map(Money::amount_minor).collect::<Vec<_>>(),
            vec![9, 8]
        );
    }

    #[test]
    fn inclusive_tax_is_extracted_without_increasing_the_total() {
        let rule = TaxRule::create("gst", "GST", "sg", 900).unwrap();
        let taxes = rule
            .calculate_and_allocate(&[Money::new(1_090, CurrencyCode::USD)], true)
            .unwrap();
        assert_eq!(taxes[0].amount_minor(), 90);
    }
}
