use crate::identity::UserId;

use super::MerchantAccountId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MerchantRole {
    Owner,
    Administrator,
    Developer,
    Manager,
    Support,
}

impl MerchantRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Administrator => "administrator",
            Self::Developer => "developer",
            Self::Manager => "manager",
            Self::Support => "support",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "administrator" => Some(Self::Administrator),
            "developer" => Some(Self::Developer),
            "manager" => Some(Self::Manager),
            "support" => Some(Self::Support),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantAccountMembership {
    merchant_account_id: MerchantAccountId,
    user_id: UserId,
    role: MerchantRole,
}

impl MerchantAccountMembership {
    pub fn owner(merchant_account_id: MerchantAccountId, user_id: UserId) -> Self {
        Self {
            merchant_account_id,
            user_id,
            role: MerchantRole::Owner,
        }
    }

    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }

    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    pub const fn role(&self) -> MerchantRole {
        self.role
    }
}
