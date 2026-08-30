use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::{SalesChannelId, StoreId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PublishableKeyId(Uuid);

impl PublishableKeyId {
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

impl Default for PublishableKeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishableKey {
    id: PublishableKeyId,
    store_id: StoreId,
    sales_channel_id: SalesChannelId,
    name: String,
}

impl PublishableKey {
    pub fn issue(
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().count() > 80 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-80 characters".into(),
            }]));
        }
        Ok(Self {
            id: PublishableKeyId::new(),
            store_id,
            sales_channel_id,
            name,
        })
    }

    pub fn from_parts(
        id: PublishableKeyId,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let mut key = Self::issue(store_id, sales_channel_id, name)?;
        key.id = id;
        Ok(key)
    }

    pub const fn id(&self) -> PublishableKeyId {
        self.id
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub const fn sales_channel_id(&self) -> SalesChannelId {
        self.sales_channel_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishable_key_accepts_a_bounded_name() {
        let key = PublishableKey::issue(StoreId::new(), SalesChannelId::new(), "Browser").unwrap();
        assert_eq!(key.name(), "Browser");
    }
}
