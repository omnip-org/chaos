use uuid::Uuid;

use crate::{DomainError, FieldViolation};

use super::UserId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct McpKeyId(Uuid);

impl McpKeyId {
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

impl Default for McpKeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpKey {
    id: McpKeyId,
    user_id: UserId,
    name: String,
}

impl McpKey {
    pub fn issue(user_id: UserId, name: impl Into<String>) -> Result<Self, DomainError> {
        let name = name.into().trim().to_owned();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "name",
                reason: "must contain 1-80 characters".into(),
            }]));
        }
        Ok(Self {
            id: McpKeyId::new(),
            user_id,
            name,
        })
    }

    pub const fn id(&self) -> McpKeyId {
        self.id
    }

    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_a_valid_key_name() {
        let key = McpKey::issue(UserId::new(), "  Store assistant  ").unwrap();
        assert_eq!(key.name(), "Store assistant");
    }

    #[test]
    fn rejects_an_empty_key_name() {
        assert!(McpKey::issue(UserId::new(), "   ").is_err());
    }
}
