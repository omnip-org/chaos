use chaos_domain::{
    identity::UserId,
    sales::ShopperId,
    store::{StoreId, StoreRole},
};
use secrecy::SecretString;

use crate::ApplicationError;
use crate::store::StoreActor;

use super::MachineActor;

/// The caller of an admin-facing use case: either a human Store member
/// (MCP OAuth access token) or a Store-scoped Publishable Key (machine client).
///
/// Kept as a closed enum rather than a trait so it stays object-safe at
/// `dyn` boundaries without
/// generics, and so the two cases are exhaustively matchable wherever
/// write/read authorization is decided.
#[derive(Clone)]
pub enum AdminActor {
    Store(StoreActor),
    Machine(MachineActor),
}

impl AdminActor {
    pub const fn store_id(&self) -> StoreId {
        match self {
            Self::Store(actor) => actor.store_id(),
            Self::Machine(actor) => actor.store_id,
        }
    }

    /// User id for RLS/audit `app.user_id`, when the caller is a signed-in
    /// human. A Publishable Key has no human behind it at request time, so
    /// this is `None` for `Machine` — callers that need a real
    /// `identity.users` row for a mutation must reject `Machine` first via
    /// [`Self::require_human`].
    pub const fn audit_user_id(&self) -> Option<UserId> {
        match self {
            Self::Store(actor) => Some(actor.user_id()),
            Self::Machine(_) => None,
        }
    }

    pub fn require_human(&self) -> Result<(), ApplicationError> {
        match self {
            Self::Store(_) => Ok(()),
            Self::Machine(_) => Err(ApplicationError::Forbidden),
        }
    }

    pub fn require_owner(&self) -> Result<(), ApplicationError> {
        match self {
            Self::Store(actor) if actor.role() == StoreRole::Owner => Ok(()),
            _ => Err(ApplicationError::Forbidden),
        }
    }
}

impl From<StoreActor> for AdminActor {
    fn from(actor: StoreActor) -> Self {
        Self::Store(actor)
    }
}

impl From<MachineActor> for AdminActor {
    fn from(actor: MachineActor) -> Self {
        Self::Machine(actor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShopperActor {
    pub machine: MachineActor,
    pub shopper_id: ShopperId,
}

impl MachineActor {
    pub fn require_sales_channel(&self) -> Result<(), ApplicationError> {
        self.channel_id
            .map(|_| ())
            .ok_or(ApplicationError::Forbidden)
    }
}

pub trait ShopperCredentialCodec: Send + Sync {
    fn issue(
        &self,
        actor: &MachineActor,
        shopper_id: ShopperId,
    ) -> Result<SecretString, ApplicationError>;

    fn verify(
        &self,
        actor: &MachineActor,
        credential: &SecretString,
    ) -> Result<ShopperId, ApplicationError>;
}
