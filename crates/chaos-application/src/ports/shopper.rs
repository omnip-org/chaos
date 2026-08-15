use chaos_domain::sales::ShopperId;
use secrecy::SecretString;

use crate::ApplicationError;

use super::MachineActor;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShopperActor {
    pub machine: MachineActor,
    pub shopper_id: ShopperId,
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
