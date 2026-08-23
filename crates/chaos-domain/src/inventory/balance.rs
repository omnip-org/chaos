// On-hand quantity and quantity transitions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryBalance {
    on_hand: i64,
}

impl InventoryBalance {
    pub fn new(on_hand: i64) -> Result<Self, DomainError> {
        if on_hand < 0 {
            return Err(validation("on_hand", "must be zero or greater"));
        }
        Ok(Self { on_hand })
    }

    pub const fn zero() -> Self {
        Self { on_hand: 0 }
    }

    pub const fn on_hand(self) -> i64 {
        self.on_hand
    }

    pub fn adjust(self, delta: i64) -> Result<Self, DomainError> {
        if delta == 0 {
            return Err(validation("delta", "must not be zero"));
        }
        let on_hand = self
            .on_hand
            .checked_add(delta)
            .ok_or_else(quantity_overflow)?;
        Self::new(on_hand)
    }
}
