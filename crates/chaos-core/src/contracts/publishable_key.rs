use chaos_domain::{
    identity::UserId,
    store::{PublishableKeyId, SalesChannelId, StoreId},
};
use time::OffsetDateTime;

pub struct GeneratedPublishableKey {
    pub public_key: String,
}

pub struct PublishableKeyListItem {
    pub id: PublishableKeyId,
    pub name: String,
    pub public_key: String,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineActor {
    pub publishable_key_id: PublishableKeyId,
    pub store_id: StoreId,
    pub sales_channel_id: Option<SalesChannelId>,
    /// The human member who created this key. Used as the audit actor for
    /// mutations that require a real `identity.users` row (e.g. Collection
    /// events) when this key drives the mutation instead of a person.
    pub created_by_user_id: UserId,
}
