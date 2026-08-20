use chaos_domain::{identity::UserId, merchant::StoreId};

use crate::merchant::StoreActor;

use super::MachineActor;

/// The caller of an admin-facing use case: either a human Store member
/// (JWT access token) or a Store-scoped API key (MCP / machine client).
///
/// Kept as a closed enum rather than a trait so it stays object-safe at
/// `dyn` port boundaries (`CatalogManagementUnitOfWork`, etc.) without
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

    /// User id for RLS/audit `app.user_id` and any audit column that hard-requires
    /// a real `identity.users` row (e.g. Collection events). For a human this is
    /// the signed-in member; for a machine it is the API key's creator
    /// (`created_by_user_id`) — the most honest stand-in available, since the
    /// mutation is genuinely attributable to the key that person issued.
    pub const fn audit_user_id(&self) -> UserId {
        match self {
            Self::Store(actor) => actor.user_id(),
            Self::Machine(actor) => actor.created_by_user_id,
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
