use std::collections::HashSet;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, FieldViolation, store::StoreId};

macro_rules! inventory_id {
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

inventory_id!(InventoryLocationId);
inventory_id!(InventoryItemId);
inventory_id!(InventoryReservationId);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InventoryLocationCode(String);

impl InventoryLocationCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (2..=32).contains(&bytes.len())
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        if !valid {
            return Err(validation(
                "code",
                "must be 2-32 lowercase ASCII letters, digits, or hyphens",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryLocation {
    id: InventoryLocationId,
    store_id: StoreId,
    code: InventoryLocationCode,
    name: String,
}

impl InventoryLocation {
    pub fn create(
        store_id: StoreId,
        code: InventoryLocationCode,
        name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(validation("name", "must contain 1-120 characters"));
        }
        Ok(Self {
            id: InventoryLocationId::new(),
            store_id,
            code,
            name,
        })
    }

    pub const fn id(&self) -> InventoryLocationId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub const fn code(&self) -> &InventoryLocationCode {
        &self.code
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StockBalance {
    on_hand: i64,
    reserved: i64,
}

impl StockBalance {
    pub fn new(on_hand: i64, reserved: i64) -> Result<Self, DomainError> {
        if on_hand < 0 {
            return Err(validation("on_hand", "must be zero or greater"));
        }
        if reserved < 0 || reserved > on_hand {
            return Err(validation(
                "reserved",
                "must be between zero and on-hand quantity",
            ));
        }
        Ok(Self { on_hand, reserved })
    }

    pub const fn zero() -> Self {
        Self {
            on_hand: 0,
            reserved: 0,
        }
    }

    pub const fn on_hand(self) -> i64 {
        self.on_hand
    }

    pub const fn reserved(self) -> i64 {
        self.reserved
    }

    pub fn available(self) -> i64 {
        self.on_hand - self.reserved
    }

    pub fn adjust(self, delta: i64) -> Result<Self, DomainError> {
        if delta == 0 {
            return Err(validation("delta", "must not be zero"));
        }
        let on_hand = self
            .on_hand
            .checked_add(delta)
            .ok_or_else(quantity_overflow)?;
        Self::new(on_hand, self.reserved)
    }

    pub fn reserve(self, quantity: i64) -> Result<Self, DomainError> {
        require_positive_quantity(quantity)?;
        if quantity > self.available() {
            return Err(validation("quantity", "exceeds available inventory"));
        }
        let reserved = self
            .reserved
            .checked_add(quantity)
            .ok_or_else(quantity_overflow)?;
        Self::new(self.on_hand, reserved)
    }

    pub fn release(self, quantity: i64) -> Result<Self, DomainError> {
        require_positive_quantity(quantity)?;
        let reserved = self
            .reserved
            .checked_sub(quantity)
            .ok_or_else(quantity_overflow)?;
        Self::new(self.on_hand, reserved)
    }

    pub fn consume(self, quantity: i64) -> Result<Self, DomainError> {
        require_positive_quantity(quantity)?;
        if quantity > self.reserved {
            return Err(validation("quantity", "exceeds reserved inventory"));
        }
        let on_hand = self
            .on_hand
            .checked_sub(quantity)
            .ok_or_else(quantity_overflow)?;
        let reserved = self
            .reserved
            .checked_sub(quantity)
            .ok_or_else(quantity_overflow)?;
        Self::new(on_hand, reserved)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryReservationStatus {
    Active,
    Released,
    Consumed,
    Expired,
}

impl InventoryReservationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            "consumed" => Some(Self::Consumed),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryReservationLine {
    inventory_item_id: InventoryItemId,
    quantity: i64,
}

impl InventoryReservationLine {
    pub fn new(inventory_item_id: InventoryItemId, quantity: i64) -> Result<Self, DomainError> {
        require_positive_quantity(quantity)?;
        Ok(Self {
            inventory_item_id,
            quantity,
        })
    }

    pub const fn inventory_item_id(&self) -> InventoryItemId {
        self.inventory_item_id
    }

    pub const fn quantity(&self) -> i64 {
        self.quantity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryReservation {
    id: InventoryReservationId,
    store_id: StoreId,
    status: InventoryReservationStatus,
    expires_at: OffsetDateTime,
    lines: Vec<InventoryReservationLine>,
}

impl InventoryReservation {
    pub fn create(
        store_id: StoreId,
        created_at: OffsetDateTime,
        expires_at: OffsetDateTime,
        lines: Vec<InventoryReservationLine>,
    ) -> Result<Self, DomainError> {
        if expires_at <= created_at {
            return Err(validation("expires_at", "must be later than creation time"));
        }
        if lines.is_empty() {
            return Err(validation("lines", "must contain at least one line"));
        }
        let unique_inventory_items = lines
            .iter()
            .map(InventoryReservationLine::inventory_item_id)
            .collect::<HashSet<_>>();
        if unique_inventory_items.len() != lines.len() {
            return Err(validation(
                "lines",
                "must contain each inventory item at most once",
            ));
        }
        Ok(Self {
            id: InventoryReservationId::new(),
            store_id,
            status: InventoryReservationStatus::Active,
            expires_at,
            lines,
        })
    }

    pub const fn id(&self) -> InventoryReservationId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub const fn status(&self) -> InventoryReservationStatus {
        self.status
    }

    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    pub fn lines(&self) -> &[InventoryReservationLine] {
        &self.lines
    }

    pub fn release(&mut self) -> Result<(), DomainError> {
        self.require_active()?;
        self.status = InventoryReservationStatus::Released;
        Ok(())
    }

    pub fn consume(&mut self, now: OffsetDateTime) -> Result<(), DomainError> {
        self.require_active()?;
        if now >= self.expires_at {
            return Err(validation("reservation", "has expired"));
        }
        self.status = InventoryReservationStatus::Consumed;
        Ok(())
    }

    pub fn expire(&mut self, now: OffsetDateTime) -> Result<(), DomainError> {
        self.require_active()?;
        if now < self.expires_at {
            return Err(validation("reservation", "has not expired"));
        }
        self.status = InventoryReservationStatus::Expired;
        Ok(())
    }

    fn require_active(&self) -> Result<(), DomainError> {
        if self.status == InventoryReservationStatus::Active {
            return Ok(());
        }
        Err(validation("reservation", "is no longer active"))
    }
}

fn require_positive_quantity(quantity: i64) -> Result<(), DomainError> {
    if quantity > 0 {
        return Ok(());
    }
    Err(validation("quantity", "must be greater than zero"))
}

fn quantity_overflow() -> DomainError {
    validation("quantity", "inventory arithmetic overflowed")
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;

    #[test]
    fn stock_balance_prevents_overselling_and_invalid_adjustments() {
        let balance = StockBalance::new(10, 3).unwrap();
        assert_eq!(balance.available(), 7);
        assert_eq!(balance.reserve(7).unwrap().reserved(), 10);
        assert!(balance.reserve(8).is_err());
        assert!(balance.adjust(-8).is_err());
        assert!(balance.adjust(0).is_err());
    }

    #[test]
    fn reservation_release_and_consumption_update_balances() {
        let reserved = StockBalance::new(10, 0).unwrap().reserve(4).unwrap();
        assert_eq!(
            reserved.release(4).unwrap(),
            StockBalance::new(10, 0).unwrap()
        );
        assert_eq!(
            reserved.consume(4).unwrap(),
            StockBalance::new(6, 0).unwrap()
        );
        assert!(reserved.consume(5).is_err());
    }

    #[test]
    fn reservation_state_transitions_are_time_aware_and_terminal() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let line = InventoryReservationLine::new(InventoryItemId::new(), 1).unwrap();
        let mut reservation = InventoryReservation::create(
            StoreId::new(),
            now,
            now + Duration::minutes(10),
            vec![line],
        )
        .unwrap();

        assert!(reservation.expire(now + Duration::minutes(9)).is_err());
        reservation.expire(now + Duration::minutes(10)).unwrap();
        assert_eq!(reservation.status(), InventoryReservationStatus::Expired);
        assert!(reservation.release().is_err());
        assert!(reservation.consume(now).is_err());
    }

    #[test]
    fn reservation_rejects_duplicate_inventory_items() {
        let inventory_item_id = InventoryItemId::new();
        let line = InventoryReservationLine::new(inventory_item_id, 1).unwrap();
        assert!(
            InventoryReservation::create(
                StoreId::new(),
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::UNIX_EPOCH + Duration::minutes(1),
                vec![line.clone(), line],
            )
            .is_err()
        );
    }
}
