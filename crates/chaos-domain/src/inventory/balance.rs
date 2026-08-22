// On-hand, reserved, available balance and quantity transitions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryBalance {
    on_hand: i64,
    reserved: i64,
}

impl InventoryBalance {
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
