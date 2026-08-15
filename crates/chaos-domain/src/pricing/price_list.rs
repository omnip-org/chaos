use std::collections::HashSet;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    CurrencyCode, DomainError, FieldViolation,
    catalog::ProductVariantId,
    merchant::{MerchantAccountId, StoreId},
};

use super::Money;

macro_rules! pricing_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

pricing_id!(PriceListId);
pricing_id!(PriceId);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PriceListCode(String);

impl PriceListCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (2..=64).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        if !valid {
            return Err(validation(
                "code",
                "must be 2-64 lowercase ASCII letters, digits, or hyphens",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriceListStatus {
    Draft,
    Active,
    Archived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PriceListSchedule {
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
}

impl PriceListSchedule {
    pub fn new(
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
    ) -> Result<Self, DomainError> {
        if starts_at
            .zip(ends_at)
            .is_some_and(|(start, end)| end <= start)
        {
            return Err(validation("ends_at", "must be later than starts_at"));
        }
        Ok(Self { starts_at, ends_at })
    }

    pub const fn unbounded() -> Self {
        Self {
            starts_at: None,
            ends_at: None,
        }
    }

    pub const fn starts_at(self) -> Option<OffsetDateTime> {
        self.starts_at
    }

    pub const fn ends_at(self) -> Option<OffsetDateTime> {
        self.ends_at
    }
}

impl PriceListStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Price {
    id: PriceId,
    product_variant_id: ProductVariantId,
    amount: Money,
}

impl Price {
    pub const fn id(&self) -> PriceId {
        self.id
    }

    pub const fn product_variant_id(&self) -> ProductVariantId {
        self.product_variant_id
    }

    pub const fn amount(&self) -> &Money {
        &self.amount
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceList {
    id: PriceListId,
    merchant_account_id: MerchantAccountId,
    store_id: StoreId,
    code: PriceListCode,
    name: String,
    currency: CurrencyCode,
    tax_inclusive: bool,
    schedule: PriceListSchedule,
    status: PriceListStatus,
    prices: Vec<Price>,
}

impl PriceList {
    pub fn create(
        merchant_account_id: MerchantAccountId,
        store_id: StoreId,
        code: PriceListCode,
        name: impl Into<String>,
        currency: CurrencyCode,
        tax_inclusive: bool,
        schedule: PriceListSchedule,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(validation("name", "must contain 1-120 characters"));
        }
        Ok(Self {
            id: PriceListId::new(),
            merchant_account_id,
            store_id,
            code,
            name,
            currency,
            tax_inclusive,
            schedule,
            status: PriceListStatus::Draft,
            prices: Vec::new(),
        })
    }

    pub fn add_price(
        &mut self,
        product_variant_id: ProductVariantId,
        amount_minor: i64,
    ) -> Result<PriceId, DomainError> {
        if amount_minor < 0 {
            return Err(validation("amount_minor", "must be zero or greater"));
        }
        if self
            .prices
            .iter()
            .any(|price| price.product_variant_id == product_variant_id)
        {
            return Err(validation(
                "product_variant_id",
                "must be unique within the price list",
            ));
        }
        let id = PriceId::new();
        self.prices.push(Price {
            id,
            product_variant_id,
            amount: Money::new(amount_minor, self.currency),
        });
        Ok(id)
    }

    pub fn activate(&mut self, active_variant_ids: &[ProductVariantId]) -> Result<(), DomainError> {
        Self::validate_activation(
            &self
                .prices
                .iter()
                .map(Price::product_variant_id)
                .collect::<Vec<_>>(),
            active_variant_ids,
        )?;
        self.status = PriceListStatus::Active;
        Ok(())
    }

    pub fn validate_activation(
        priced_variant_ids: &[ProductVariantId],
        active_variant_ids: &[ProductVariantId],
    ) -> Result<(), DomainError> {
        if priced_variant_ids.is_empty() {
            return Err(validation(
                "prices",
                "must contain at least one price before activation",
            ));
        }
        let active = active_variant_ids.iter().copied().collect::<HashSet<_>>();
        if priced_variant_ids
            .iter()
            .any(|variant_id| !active.contains(variant_id))
        {
            return Err(validation(
                "prices",
                "may reference only active product variants",
            ));
        }
        Ok(())
    }

    pub fn archive(&mut self) {
        self.status = PriceListStatus::Archived;
    }

    pub const fn id(&self) -> PriceListId {
        self.id
    }
    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }
    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }
    pub const fn code(&self) -> &PriceListCode {
        &self.code
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn currency(&self) -> CurrencyCode {
        self.currency
    }
    pub const fn tax_inclusive(&self) -> bool {
        self.tax_inclusive
    }
    pub const fn starts_at(&self) -> Option<OffsetDateTime> {
        self.schedule.starts_at()
    }
    pub const fn ends_at(&self) -> Option<OffsetDateTime> {
        self.schedule.ends_at()
    }
    pub const fn status(&self) -> PriceListStatus {
        self.status
    }
    pub fn prices(&self) -> &[Price] {
        &self.prices
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

    fn price_list() -> PriceList {
        PriceList::create(
            MerchantAccountId::new(),
            StoreId::new(),
            PriceListCode::parse("us-retail").unwrap(),
            "US Retail",
            CurrencyCode::USD,
            false,
            PriceListSchedule::unbounded(),
        )
        .unwrap()
    }

    #[test]
    fn price_list_starts_as_an_incomplete_draft() {
        let mut list = price_list();
        assert_eq!(list.status(), PriceListStatus::Draft);
        assert!(list.activate(&[]).is_err());
    }

    #[test]
    fn variant_is_priced_once_per_list_with_list_currency() {
        let mut list = price_list();
        let variant_id = ProductVariantId::new();
        list.add_price(variant_id, 2_500).unwrap();
        assert_eq!(
            list.prices()[0].amount(),
            &Money::new(2_500, CurrencyCode::USD)
        );
        assert!(list.add_price(variant_id, 2_600).is_err());
        assert!(list.add_price(ProductVariantId::new(), -1).is_err());
    }

    #[test]
    fn activation_requires_every_priced_variant_to_be_active() {
        let mut list = price_list();
        let variant_id = ProductVariantId::new();
        list.add_price(variant_id, 2_500).unwrap();
        assert!(list.activate(&[ProductVariantId::new()]).is_err());
        list.activate(&[variant_id]).unwrap();
        assert_eq!(list.status(), PriceListStatus::Active);
    }

    #[test]
    fn archive_is_an_explicit_lifecycle_transition() {
        let mut list = price_list();
        list.archive();
        assert_eq!(list.status(), PriceListStatus::Archived);
    }
}
