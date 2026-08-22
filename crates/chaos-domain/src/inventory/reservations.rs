// Inventory reservation aggregate, lines, and status transitions.

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
