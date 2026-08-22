// Inventory location aggregate.

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
