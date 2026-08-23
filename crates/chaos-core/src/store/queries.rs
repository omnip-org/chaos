use std::sync::Arc;

use chaos_domain::{
    identity::{AccessKeyId, UserId},
    store::{StoreId, StoreRole},
};

use crate::{
    ApplicationError,
    ports::{StoreListItem, StoreReadRepository},
};

pub struct Page<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

#[derive(Clone, Copy)]
pub struct StoreActor {
    user_id: UserId,
    store_id: StoreId,
    role: StoreRole,
    access_key_id: Option<AccessKeyId>,
}

impl StoreActor {
    pub(crate) const fn new(user_id: UserId, store_id: StoreId, role: StoreRole) -> Self {
        Self {
            user_id,
            store_id,
            role,
            access_key_id: None,
        }
    }

    pub const fn with_access_key(mut self, key_id: AccessKeyId) -> Self {
        self.access_key_id = Some(key_id);
        self
    }

    pub const fn user_id(self) -> UserId {
        self.user_id
    }

    pub const fn store_id(self) -> StoreId {
        self.store_id
    }

    pub const fn role(self) -> StoreRole {
        self.role
    }

    pub const fn access_key_id(self) -> Option<AccessKeyId> {
        self.access_key_id
    }
}

pub struct StoreQueries {
    repository: Arc<dyn StoreReadRepository>,
}

impl StoreQueries {
    pub fn new(repository: Arc<dyn StoreReadRepository>) -> Self {
        Self { repository }
    }

    pub async fn authorize(
        &self,
        user_id: UserId,
        store_id: StoreId,
    ) -> Result<StoreActor, ApplicationError> {
        let role = self
            .repository
            .membership_role(user_id, store_id)
            .await?
            .ok_or(ApplicationError::Forbidden)?;
        Ok(StoreActor::new(user_id, store_id, role))
    }

    pub async fn list_stores(
        &self,
        user_id: UserId,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Page<StoreListItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_stores(user_id, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }
}
