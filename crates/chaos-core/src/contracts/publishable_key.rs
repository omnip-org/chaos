use chaos_domain::store::{PublishableKeyId, SalesChannelId, StoreId};
use time::OffsetDateTime;

pub struct GeneratedPublishableKey {
    pub public_key: String,
}

pub struct PublishableKeyListItem {
    pub id: PublishableKeyId,
    pub sales_channel_id: SalesChannelId,
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
}
