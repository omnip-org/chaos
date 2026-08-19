use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{MachineActor, ShopperCredentialCodec},
};
use chaos_domain::{merchant::SalesChannelId, sales::ShopperId};
use hmac::{Hmac, KeyInit, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

const TOKEN_PREFIX: &str = "cc_shopper_v1";

pub struct HmacShopperCredentialCodec {
    active_key_id: String,
    keys: HashMap<String, Vec<u8>>,
}

impl HmacShopperCredentialCodec {
    pub fn new(
        active_key_id: impl Into<String>,
        active_secret: impl Into<Vec<u8>>,
        previous_key: Option<(String, Vec<u8>)>,
    ) -> anyhow::Result<Self> {
        let active_key_id = validate_key_id(active_key_id.into())?;
        let active_secret = validate_secret(active_secret.into())?;
        let mut keys = HashMap::from([(active_key_id.clone(), active_secret)]);
        if let Some((key_id, secret)) = previous_key {
            let key_id = validate_key_id(key_id)?;
            if keys.contains_key(&key_id) {
                anyhow::bail!("shopper token key identifiers must be unique");
            }
            keys.insert(key_id, validate_secret(secret)?);
        }
        Ok(Self {
            active_key_id,
            keys,
        })
    }

    fn signature(
        &self,
        key_id: &str,
        shopper_id: Uuid,
        store_id: Uuid,
        sales_channel_id: Uuid,
    ) -> Result<Vec<u8>, ApplicationError> {
        let secret = self
            .keys
            .get(key_id)
            .ok_or(ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(signing_input(key_id, shopper_id, store_id, sales_channel_id).as_bytes());
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
            &self.active_key_id,
            shopper_id.as_uuid(),
            actor.store_id.as_uuid(),
            sales_channel_id.as_uuid(),
        )?;
        Ok(SecretString::from(format!(
            "{TOKEN_PREFIX}.{}.{}.{}.{}.{}",
            self.active_key_id,
            shopper_id.as_uuid().simple(),
            actor.store_id.as_uuid().simple(),
            sales_channel_id.as_uuid().simple(),
            URL_SAFE_NO_PAD.encode(signature)
        )))
    }

    fn verify(
        &self,
        actor: &MachineActor,
        credential: &SecretString,
    ) -> Result<ShopperId, ApplicationError> {
        let parts = credential.expose_secret().split('.').collect::<Vec<_>>();
        if parts.len() != 6 || parts[0] != TOKEN_PREFIX {
            return Err(ApplicationError::Unauthorized);
        }
        let key_id = parts[1];
        let shopper_id = Uuid::parse_str(parts[2]).map_err(|_| ApplicationError::Unauthorized)?;
        let store_id = Uuid::parse_str(parts[3]).map_err(|_| ApplicationError::Unauthorized)?;
        let sales_channel_id =
            Uuid::parse_str(parts[4]).map_err(|_| ApplicationError::Unauthorized)?;
        if store_id != actor.store_id.as_uuid()
            || Some(SalesChannelId::from_uuid(sales_channel_id)) != actor.sales_channel_id
        {
            return Err(ApplicationError::Unauthorized);
        }
        let presented = URL_SAFE_NO_PAD
            .decode(parts[5])
            .map_err(|_| ApplicationError::Unauthorized)?;
        let secret = self
            .keys
            .get(key_id)
            .ok_or(ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(signing_input(key_id, shopper_id, store_id, sales_channel_id).as_bytes());
        mac.verify_slice(&presented)
            .map_err(|_| ApplicationError::Unauthorized)?;
        Ok(ShopperId::from_uuid(shopper_id))
    }
}

fn signing_input(key_id: &str, shopper_id: Uuid, store_id: Uuid, sales_channel_id: Uuid) -> String {
    format!("{TOKEN_PREFIX}:{key_id}:{shopper_id}:{store_id}:{sales_channel_id}")
}

fn validate_key_id(value: String) -> anyhow::Result<String> {
    if !(1..=16).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!(
            "shopper token key identifier must be 1-16 ASCII letters, digits, or hyphens"
        );
    }
    Ok(value)
}

fn validate_secret(value: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if value.len() < 32 {
        anyhow::bail!("shopper token secret must contain at least 32 bytes");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use chaos_application::ports::{MachineActor, ShopperCredentialCodec};
    use chaos_domain::{
        identity::UserId,
        merchant::{ApiKeyClass, ApiKeyId, ApiKeyScope, SalesChannelId, StoreId},
        sales::ShopperId,
    };
    use secrecy::SecretString;

    use super::HmacShopperCredentialCodec;

    fn actor() -> MachineActor {
        MachineActor {
            api_key_id: ApiKeyId::new(),
            store_id: StoreId::new(),
            sales_channel_id: Some(SalesChannelId::new()),
            class: ApiKeyClass::Publishable,
            scopes: vec![ApiKeyScope::CartsWrite],
            created_by_user_id: UserId::new(),
        }
    }

    #[test]
    fn issues_verifies_and_rejects_modified_credentials() {
        let codec = HmacShopperCredentialCodec::new("current", [7_u8; 32], None).unwrap();
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
    fn accepts_the_previous_rotation_key() {
        let old = HmacShopperCredentialCodec::new("old", [5_u8; 32], None).unwrap();
        let actor = actor();
        let shopper_id = ShopperId::new();
        let credential = old.issue(&actor, shopper_id).unwrap();
        let rotated = HmacShopperCredentialCodec::new(
            "new",
            [7_u8; 32],
            Some(("old".into(), [5_u8; 32].to_vec())),
        )
        .unwrap();

        assert_eq!(rotated.verify(&actor, &credential).unwrap(), shopper_id);
    }
}
