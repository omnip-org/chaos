use std::sync::Arc;

use chaos_domain::{
    identity::UserId,
    merchant::{MerchantAccountId, MerchantRole, StoreId},
};

use crate::{
    ApplicationError,
    ports::{MerchantAccountListItem, MerchantReadRepository, StoreListItem},
};

pub struct Page<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

#[derive(Clone, Copy)]
pub struct MerchantActor {
    user_id: UserId,
    merchant_account_id: MerchantAccountId,
    role: MerchantRole,
}

impl MerchantActor {
    pub const fn user_id(self) -> UserId {
        self.user_id
    }

    pub const fn merchant_account_id(self) -> MerchantAccountId {
        self.merchant_account_id
    }

    pub const fn role(self) -> MerchantRole {
        self.role
    }
}

pub struct MerchantQueries {
    repository: Arc<dyn MerchantReadRepository>,
}

impl MerchantQueries {
    pub fn new(repository: Arc<dyn MerchantReadRepository>) -> Self {
        Self { repository }
    }

    pub async fn list_merchant_accounts(
        &self,
        user_id: UserId,
        after: Option<MerchantAccountId>,
        limit: u16,
    ) -> Result<Page<MerchantAccountListItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_merchant_accounts(user_id, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn authorize(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<MerchantActor, ApplicationError> {
        let role = self
            .repository
            .membership_role(user_id, merchant_account_id)
            .await?
            .ok_or(ApplicationError::Forbidden)?;
        Ok(MerchantActor {
            user_id,
            merchant_account_id,
            role,
        })
    }

    pub async fn list_stores(
        &self,
        actor: MerchantActor,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Page<StoreListItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_stores(actor.user_id, actor.merchant_account_id, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }
}
