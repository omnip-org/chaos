use std::sync::Arc;

use chaos_domain::{
    identity::UserId,
    store::{StoreId, StoreRole},
};

use crate::{
    ApplicationError,
    contracts::{StoreMembershipItem, StoreMembershipRepository},
};

use super::StoreActor;

pub struct StoreMembershipManagement {
    repository: Arc<dyn StoreMembershipRepository>,
}

impl StoreMembershipManagement {
    pub fn new(repository: Arc<dyn StoreMembershipRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Vec<StoreMembershipItem>, ApplicationError> {
        self.repository.list(actor, store_id).await
    }

    pub async fn add_member(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
    ) -> Result<StoreMembershipItem, ApplicationError> {
        require_owner(actor)?;
        self.repository.add_member(actor, store_id, user_id).await
    }

    pub async fn set_role(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
        role: StoreRole,
    ) -> Result<StoreMembershipItem, ApplicationError> {
        require_owner(actor)?;
        self.repository
            .set_role(actor, store_id, user_id, role)
            .await
    }

    pub async fn leave(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<(), ApplicationError> {
        self.repository.leave(actor, store_id).await
    }
}

fn require_owner(actor: StoreActor) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use time::OffsetDateTime;

    use super::*;

    struct RecordingRepository(AtomicUsize);

    #[async_trait]
    impl StoreMembershipRepository for RecordingRepository {
        async fn list(
            &self,
            _actor: StoreActor,
            _store_id: StoreId,
        ) -> Result<Vec<StoreMembershipItem>, ApplicationError> {
            Ok(Vec::new())
        }

        async fn add_member(
            &self,
            _actor: StoreActor,
            _store_id: StoreId,
            user_id: UserId,
        ) -> Result<StoreMembershipItem, ApplicationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(StoreMembershipItem {
                user_id,
                role: StoreRole::Member,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            })
        }

        async fn set_role(
            &self,
            _actor: StoreActor,
            _store_id: StoreId,
            _user_id: UserId,
            _role: StoreRole,
        ) -> Result<StoreMembershipItem, ApplicationError> {
            unreachable!()
        }

        async fn leave(
            &self,
            _actor: StoreActor,
            _store_id: StoreId,
        ) -> Result<(), ApplicationError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn only_an_owner_can_add_a_store_member() {
        let repository = Arc::new(RecordingRepository(AtomicUsize::new(0)));
        let service = StoreMembershipManagement::new(repository.clone());
        let store_id = StoreId::new();
        let owner_id = UserId::new();

        let member = StoreActor::new(UserId::new(), store_id, StoreRole::Member);
        assert!(matches!(
            service.add_member(member, store_id, owner_id).await,
            Err(ApplicationError::Forbidden)
        ));
        assert_eq!(repository.0.load(Ordering::SeqCst), 0);

        let owner = StoreActor::new(UserId::new(), store_id, StoreRole::Owner);
        let added = service.add_member(owner, store_id, owner_id).await.unwrap();
        assert_eq!(added.user_id, owner_id);
        assert_eq!(repository.0.load(Ordering::SeqCst), 1);
    }
}
