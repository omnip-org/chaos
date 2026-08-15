use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CurrencyCode, DomainError, FieldViolation};

use super::Money;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PromotionId(Uuid);

impl PromotionId {
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

impl Default for PromotionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromotionStatus {
    Active,
    Archived,
}

impl PromotionStatus {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromotionTrigger {
    Automatic,
    Code,
}

impl PromotionTrigger {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Code => "code",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "automatic" => Some(Self::Automatic),
            "code" => Some(Self::Code),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromotionValue {
    Percentage {
        rate_basis_points: u32,
        maximum_amount_minor: Option<i64>,
    },
    FixedAmount {
        amount_minor: i64,
    },
}

impl PromotionValue {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Percentage { .. } => "percentage",
            Self::FixedAmount { .. } => "fixed_amount",
        }
    }
    pub const fn rate_basis_points(&self) -> Option<u32> {
        match self {
            Self::Percentage {
                rate_basis_points, ..
            } => Some(*rate_basis_points),
            Self::FixedAmount { .. } => None,
        }
    }
    pub const fn amount_minor(&self) -> Option<i64> {
        match self {
            Self::FixedAmount { amount_minor } => Some(*amount_minor),
            Self::Percentage { .. } => None,
        }
    }
    pub const fn maximum_amount_minor(&self) -> Option<i64> {
        match self {
            Self::Percentage {
                maximum_amount_minor,
                ..
            } => *maximum_amount_minor,
            Self::FixedAmount { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Promotion {
    id: PromotionId,
    handle: String,
    name: String,
    trigger: PromotionTrigger,
    redemption_code: Option<String>,
    value: PromotionValue,
    currency: CurrencyCode,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
    status: PromotionStatus,
}

impl Promotion {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        handle: impl Into<String>,
        name: impl Into<String>,
        trigger: PromotionTrigger,
        redemption_code: Option<String>,
        value: PromotionValue,
        currency: CurrencyCode,
        minimum_subtotal_amount_minor: i64,
        priority: u16,
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
    ) -> Result<Self, DomainError> {
        Self::rehydrate(
            PromotionId::new(),
            handle,
            name,
            trigger,
            redemption_code,
            value,
            currency,
            minimum_subtotal_amount_minor,
            priority,
            starts_at,
            ends_at,
            PromotionStatus::Active,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rehydrate(
        id: PromotionId,
        handle: impl Into<String>,
        name: impl Into<String>,
        trigger: PromotionTrigger,
        redemption_code: Option<String>,
        value: PromotionValue,
        currency: CurrencyCode,
        minimum_subtotal_amount_minor: i64,
        priority: u16,
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
        status: PromotionStatus,
    ) -> Result<Self, DomainError> {
        let handle = handle.into();
        let name = name.into();
        if handle.is_empty()
            || handle.len() > 64
            || !handle
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(validation(
                "handle",
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
        let redemption_code = redemption_code.map(|code| code.trim().to_ascii_uppercase());
        match (trigger, redemption_code.as_deref()) {
            (PromotionTrigger::Automatic, None) => {}
            (PromotionTrigger::Code, Some(code))
                if !code.is_empty()
                    && code.len() <= 64
                    && code.bytes().all(|byte| {
                        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'
                    }) => {}
            (PromotionTrigger::Automatic, Some(_)) => {
                return Err(validation(
                    "redemption_code",
                    "must be absent for automatic promotions",
                ));
            }
            (PromotionTrigger::Code, _) => {
                return Err(validation(
                    "redemption_code",
                    "must contain 1-64 uppercase letters, digits, or hyphens",
                ));
            }
        }
        match value {
            PromotionValue::Percentage {
                rate_basis_points,
                maximum_amount_minor,
            } if !(1..=10_000).contains(&rate_basis_points)
                || maximum_amount_minor.is_some_and(|amount| amount <= 0) =>
            {
                return Err(validation(
                    "value",
                    "percentage must be 1-10000 basis points with a positive optional maximum",
                ));
            }
            PromotionValue::FixedAmount { amount_minor } if amount_minor <= 0 => {
                return Err(validation("value", "fixed amount must be positive"));
            }
            _ => {}
        }
        if minimum_subtotal_amount_minor < 0 {
            return Err(validation(
                "minimum_subtotal_amount_minor",
                "must be zero or greater",
            ));
        }
        if priority > 32_767 {
            return Err(validation("priority", "must be between 0 and 32767"));
        }
        if starts_at
            .zip(ends_at)
            .is_some_and(|(start, end)| start >= end)
        {
            return Err(validation("ends_at", "must be later than starts_at"));
        }
        Ok(Self {
            id,
            handle,
            name,
            trigger,
            redemption_code,
            value,
            currency,
            minimum_subtotal_amount_minor,
            priority,
            starts_at,
            ends_at,
            status,
        })
    }

    pub const fn id(&self) -> PromotionId {
        self.id
    }
    pub fn handle(&self) -> &str {
        &self.handle
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn trigger(&self) -> PromotionTrigger {
        self.trigger
    }
    pub fn redemption_code(&self) -> Option<&str> {
        self.redemption_code.as_deref()
    }
    pub const fn value(&self) -> &PromotionValue {
        &self.value
    }
    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }
    pub const fn minimum_subtotal_amount_minor(&self) -> i64 {
        self.minimum_subtotal_amount_minor
    }
    pub const fn priority(&self) -> u16 {
        self.priority
    }
    pub const fn starts_at(&self) -> Option<OffsetDateTime> {
        self.starts_at
    }
    pub const fn ends_at(&self) -> Option<OffsetDateTime> {
        self.ends_at
    }
    pub const fn status(&self) -> PromotionStatus {
        self.status
    }

    pub fn calculate_and_allocate(
        &self,
        subtotals: &[Money],
        now: OffsetDateTime,
    ) -> Result<Option<Vec<Money>>, DomainError> {
        let Some(first) = subtotals.first() else {
            return Err(validation("subtotals", "must not be empty"));
        };
        if subtotals
            .iter()
            .any(|subtotal| subtotal.currency() != self.currency || subtotal.amount_minor() < 0)
        {
            return Err(validation(
                "subtotals",
                "must be non-negative and use the Promotion currency",
            ));
        }
        let total = subtotals.iter().try_fold(0_i128, |sum, subtotal| {
            sum.checked_add(i128::from(subtotal.amount_minor()))
                .ok_or_else(arithmetic_overflow)
        })?;
        if self.status != PromotionStatus::Active
            || self.starts_at.is_some_and(|start| now < start)
            || self.ends_at.is_some_and(|end| now >= end)
            || total < i128::from(self.minimum_subtotal_amount_minor)
            || total == 0
        {
            return Ok(None);
        }
        let discount = match self.value {
            PromotionValue::Percentage {
                rate_basis_points,
                maximum_amount_minor,
            } => {
                let rounded = total
                    .checked_mul(i128::from(rate_basis_points))
                    .ok_or_else(arithmetic_overflow)?
                    .checked_add(5_000)
                    .ok_or_else(arithmetic_overflow)?
                    / 10_000;
                maximum_amount_minor
                    .map(i128::from)
                    .map_or(rounded, |maximum| rounded.min(maximum))
                    .min(total)
            }
            PromotionValue::FixedAmount { amount_minor } => i128::from(amount_minor).min(total),
        };
        let discount = i64::try_from(discount).map_err(|_| arithmetic_overflow())?;
        if discount == 0 {
            return Ok(None);
        }
        let weights = subtotals
            .iter()
            .map(|subtotal| {
                u64::try_from(subtotal.amount_minor()).map_err(|_| arithmetic_overflow())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(
            Money::new(discount, first.currency()).allocate(&weights)?,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromotionSnapshot {
    promotion_id: PromotionId,
    handle: String,
    name: String,
    trigger: PromotionTrigger,
    redemption_code: Option<String>,
    value: PromotionValue,
    currency: CurrencyCode,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
}

impl PromotionSnapshot {
    pub fn from_promotion(promotion: &Promotion) -> Self {
        Self {
            promotion_id: promotion.id,
            handle: promotion.handle.clone(),
            name: promotion.name.clone(),
            trigger: promotion.trigger,
            redemption_code: promotion.redemption_code.clone(),
            value: promotion.value.clone(),
            currency: promotion.currency,
            minimum_subtotal_amount_minor: promotion.minimum_subtotal_amount_minor,
            priority: promotion.priority,
            starts_at: promotion.starts_at,
            ends_at: promotion.ends_at,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn rehydrate(
        promotion_id: PromotionId,
        handle: impl Into<String>,
        name: impl Into<String>,
        trigger: PromotionTrigger,
        redemption_code: Option<String>,
        value: PromotionValue,
        currency: CurrencyCode,
        minimum_subtotal_amount_minor: i64,
        priority: u16,
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
    ) -> Result<Self, DomainError> {
        let promotion = Promotion::rehydrate(
            promotion_id,
            handle,
            name,
            trigger,
            redemption_code,
            value,
            currency,
            minimum_subtotal_amount_minor,
            priority,
            starts_at,
            ends_at,
            PromotionStatus::Active,
        )?;
        Ok(Self::from_promotion(&promotion))
    }
    pub const fn promotion_id(&self) -> PromotionId {
        self.promotion_id
    }
    pub fn handle(&self) -> &str {
        &self.handle
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn trigger(&self) -> PromotionTrigger {
        self.trigger
    }
    pub fn redemption_code(&self) -> Option<&str> {
        self.redemption_code.as_deref()
    }
    pub const fn value(&self) -> &PromotionValue {
        &self.value
    }
    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }
    pub const fn minimum_subtotal_amount_minor(&self) -> i64 {
        self.minimum_subtotal_amount_minor
    }
    pub const fn priority(&self) -> u16 {
        self.priority
    }
    pub const fn starts_at(&self) -> Option<OffsetDateTime> {
        self.starts_at
    }
    pub const fn ends_at(&self) -> Option<OffsetDateTime> {
        self.ends_at
    }
}

fn arithmetic_overflow() -> DomainError {
    validation("amount_minor", "promotion arithmetic overflowed")
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

    fn percentage(rate_basis_points: u32) -> Promotion {
        Promotion::create(
            "summer",
            "Summer sale",
            PromotionTrigger::Automatic,
            None,
            PromotionValue::Percentage {
                rate_basis_points,
                maximum_amount_minor: None,
            },
            CurrencyCode::USD,
            0,
            100,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn rounds_once_and_allocates_deterministically() {
        let allocations = percentage(825)
            .calculate_and_allocate(
                &[
                    Money::new(100, CurrencyCode::USD),
                    Money::new(100, CurrencyCode::USD),
                ],
                OffsetDateTime::UNIX_EPOCH,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            allocations
                .iter()
                .map(Money::amount_minor)
                .collect::<Vec<_>>(),
            vec![9, 8]
        );
    }

    #[test]
    fn fixed_discount_is_capped_at_subtotal() {
        let promotion = Promotion::create(
            "credit",
            "Store credit",
            PromotionTrigger::Code,
            Some("save".into()),
            PromotionValue::FixedAmount { amount_minor: 500 },
            CurrencyCode::USD,
            0,
            100,
            None,
            None,
        )
        .unwrap();
        let allocations = promotion
            .calculate_and_allocate(
                &[
                    Money::new(120, CurrencyCode::USD),
                    Money::new(80, CurrencyCode::USD),
                ],
                OffsetDateTime::UNIX_EPOCH,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            allocations
                .iter()
                .map(Money::amount_minor)
                .collect::<Vec<_>>(),
            vec![120, 80]
        );
    }

    #[test]
    fn eligibility_respects_minimum_and_half_open_window() {
        let promotion = Promotion::create(
            "window",
            "Window",
            PromotionTrigger::Automatic,
            None,
            PromotionValue::Percentage {
                rate_basis_points: 1000,
                maximum_amount_minor: Some(50),
            },
            CurrencyCode::USD,
            100,
            0,
            Some(OffsetDateTime::UNIX_EPOCH),
            Some(OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)),
        )
        .unwrap();
        assert!(
            promotion
                .calculate_and_allocate(
                    &[Money::new(99, CurrencyCode::USD)],
                    OffsetDateTime::UNIX_EPOCH
                )
                .unwrap()
                .is_none()
        );
        assert!(
            percentage(1)
                .calculate_and_allocate(
                    &[Money::new(1, CurrencyCode::USD)],
                    OffsetDateTime::UNIX_EPOCH,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            promotion
                .calculate_and_allocate(
                    &[Money::new(100, CurrencyCode::USD)],
                    OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn validates_trigger_and_value_shapes() {
        assert!(
            Promotion::create(
                "bad",
                "Bad",
                PromotionTrigger::Code,
                None,
                PromotionValue::Percentage {
                    rate_basis_points: 0,
                    maximum_amount_minor: None
                },
                CurrencyCode::USD,
                0,
                0,
                None,
                None
            )
            .is_err()
        );
    }
}
