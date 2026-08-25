use crate::{
    ApplicationError,
    contracts::{MachineActor, ShopperCredentialCodec},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_domain::sales::ShopperId;
use hmac::{Hmac, KeyInit, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

const TOKEN_PREFIX: &str = "shopper";

pub struct HmacShopperCredentialCodec {
    secret: Vec<u8>,
}

impl HmacShopperCredentialCodec {
    pub fn new(secret: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        Ok(Self {
            secret: validate_secret(secret.into())?,
        })
    }

    fn signature(
        &self,
        shopper_id: Uuid,
        store_id: Uuid,
        sales_channel_id: Uuid,
    ) -> Result<Vec<u8>, ApplicationError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(signing_input(shopper_id, store_id, sales_channel_id).as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

impl ShopperCredentialCodec for HmacShopperCredentialCodec {
    fn issue(
        &self,
        actor: &MachineActor,
        shopper_id: ShopperId,
    ) -> Result<SecretString, ApplicationError> {
        let sales_channel_id = actor
            .sales_channel_id
            .ok_or(ApplicationError::Unauthorized)?;
        let signature = self.signature(
            shopper_id.as_uuid(),
            actor.store_id.as_uuid(),
            sales_channel_id.as_uuid(),
        )?;
        Ok(SecretString::from(format!(
            "{TOKEN_PREFIX}.{}.{}",
            shopper_id.as_uuid().simple(),
            URL_SAFE_NO_PAD.encode(signature)
        )))
    }

    fn verify(
        &self,
        actor: &MachineActor,
        credential: &SecretString,
    ) -> Result<ShopperId, ApplicationError> {
        let parts = credential.expose_secret().split('.').collect::<Vec<_>>();
        if parts.len() != 3 || parts[0] != TOKEN_PREFIX {
            return Err(ApplicationError::Unauthorized);
        }
        let shopper_id = Uuid::parse_str(parts[1]).map_err(|_| ApplicationError::Unauthorized)?;
        let sales_channel_id = actor
            .sales_channel_id
            .ok_or(ApplicationError::Unauthorized)?;
        let presented = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(
            signing_input(
                shopper_id,
                actor.store_id.as_uuid(),
                sales_channel_id.as_uuid(),
            )
            .as_bytes(),
        );
        mac.verify_slice(&presented)
            .map_err(|_| ApplicationError::Unauthorized)?;
        Ok(ShopperId::from_uuid(shopper_id))
    }
}

fn signing_input(shopper_id: Uuid, store_id: Uuid, sales_channel_id: Uuid) -> String {
    format!("{TOKEN_PREFIX}:{shopper_id}:{store_id}:{sales_channel_id}")
}

fn validate_secret(value: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if value.len() < 32 {
        anyhow::bail!("shopper token secret must contain at least 32 bytes");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use crate::contracts::{MachineActor, ShopperCredentialCodec};
    use chaos_domain::{
        sales::ShopperId,
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
    use secrecy::SecretString;

    use super::HmacShopperCredentialCodec;

    fn actor() -> MachineActor {
        MachineActor {
            publishable_key_id: PublishableKeyId::new(),
            store_id: StoreId::new(),
            sales_channel_id: Some(SalesChannelId::new()),
        }
    }

    #[test]
    fn issues_verifies_and_rejects_modified_credentials() {
        let codec = HmacShopperCredentialCodec::new([7_u8; 32]).unwrap();
        let actor = actor();
        let shopper_id = ShopperId::new();
        let credential = codec.issue(&actor, shopper_id).unwrap();
        assert_eq!(codec.verify(&actor, &credential).unwrap(), shopper_id);

        let modified = SecretString::from(format!(
            "{}x",
            secrecy::ExposeSecret::expose_secret(&credential)
        ));
        assert!(codec.verify(&actor, &modified).is_err());
        let other_store = MachineActor {
            store_id: StoreId::new(),
            ..actor.clone()
        };
        assert!(codec.verify(&other_store, &credential).is_err());
    }

    #[test]
    fn rejects_a_token_for_another_sales_channel() {
        let codec = HmacShopperCredentialCodec::new([7_u8; 32]).unwrap();
        let actor = actor();
        let shopper_id = ShopperId::new();
        let credential = codec.issue(&actor, shopper_id).unwrap();
        let other_channel = MachineActor {
            sales_channel_id: Some(SalesChannelId::new()),
            ..actor
        };

        assert!(codec.verify(&other_channel, &credential).is_err());
    }
}
