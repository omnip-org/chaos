use std::sync::Arc;

use crate::{
    ApplicationError,
    adapters::postgres::DefaultPublishableKeyGenerator,
    adapters::postgres::PostgresPublishableKeyRepository,
    contracts::{AdminActor, MachineActor, PublishableKeyListItem},
};
use chaos_domain::store::{PublishableKey, PublishableKeyId, SalesChannelId, StoreId, StoreRole};

use super::Page;

pub struct CreatePublishableKeyInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub channel_id: SalesChannelId,
    pub name: String,
}

pub struct CreatePublishableKeyOutput {
    pub publishable_key: PublishableKey,
    pub public_key: String,
}

pub struct PublishableKeyManagement {
    repository: Arc<PostgresPublishableKeyRepository>,
    generator: Arc<DefaultPublishableKeyGenerator>,
}

impl PublishableKeyManagement {
    pub fn new(
        repository: Arc<PostgresPublishableKeyRepository>,
        generator: Arc<DefaultPublishableKeyGenerator>,
    ) -> Self {
        Self {
            repository,
            generator,
        }
    }

    pub async fn create(
        &self,
        input: CreatePublishableKeyInput,
    ) -> Result<CreatePublishableKeyOutput, ApplicationError> {
        authorize_publishable_key_management(&input.actor)?;
        let publishable_key = PublishableKey::issue(input.store_id, input.channel_id, input.name)?;
        let material = self.generator.generate();
        let (publishable_key_id, public_key) = self
            .repository
            .create(input.actor, &publishable_key, &material)
            .await?;
        let publishable_key = if publishable_key_id == publishable_key.id() {
            publishable_key
        } else {
            PublishableKey::from_parts(
                publishable_key_id,
                input.store_id,
                input.channel_id,
                publishable_key.name().to_owned(),
            )?
        };

        Ok(CreatePublishableKeyOutput {
            publishable_key,
            public_key,
        })
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PublishableKeyId>,
        limit: u16,
    ) -> Result<Page<PublishableKeyListItem>, ApplicationError> {
        authorize_publishable_key_management(&actor)?;
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list(actor, store_id, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn revoke(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        publishable_key_id: PublishableKeyId,
    ) -> Result<(), ApplicationError> {
        authorize_publishable_key_management(&actor)?;
        self.repository
            .revoke(actor, store_id, publishable_key_id)
            .await
    }
}

pub struct PublishableKeyAuthentication {
    repository: Arc<PostgresPublishableKeyRepository>,
}

impl PublishableKeyAuthentication {
    pub fn new(repository: Arc<PostgresPublishableKeyRepository>) -> Self {
        Self { repository }
    }

    pub async fn authenticate(
        &self,
        presented_key: &str,
    ) -> Result<MachineActor, ApplicationError> {
        let actor = self
            .repository
            .authenticate(presented_key)
            .await?
            .ok_or(ApplicationError::Unauthorized)?;
        Ok(actor)
    }
}

fn authorize_publishable_key_management(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(store_actor) => match store_actor.role() {
            StoreRole::Owner => Ok(()),
            StoreRole::Member => Err(ApplicationError::Forbidden),
        },
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::identity::UserId;

    use super::*;
    use crate::store::StoreActor;

    fn actor(role: StoreRole) -> AdminActor {
        AdminActor::Store(StoreActor::new(UserId::new(), StoreId::new(), role))
    }

    #[test]
    fn only_credential_administrators_can_manage_publishable_keys() {
        assert!(authorize_publishable_key_management(&actor(StoreRole::Owner)).is_ok());
        assert!(matches!(
            authorize_publishable_key_management(&actor(StoreRole::Member)),
            Err(ApplicationError::Forbidden)
        ));
    }
}
