use crate::identity::UserId;

use super::StoreId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreRole {
    Owner,
    Member,
}

impl StoreRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreMembership {
    store_id: StoreId,
    user_id: UserId,
    role: StoreRole,
}

impl StoreMembership {
    pub fn owner(store_id: StoreId, user_id: UserId) -> Self {
        Self {
            store_id,
            user_id,
            role: StoreRole::Owner,
        }
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    pub const fn role(&self) -> StoreRole {
        self.role
    }
}
