use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_domain::sales::OrderId;
use hmac::{Hmac, KeyInit, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ApplicationError,
    contracts::{CheckoutActor, CheckoutCredentialCodec, MachineActor},
};

const TOKEN_PREFIX: &str = "checkout";
// Stripe Checkout Sessions normally remain open for up to 24 hours. Keep the
// recovery capability valid for another day so a delayed expiration webhook
// can still resolve the shared link to its terminal Order.
const TOKEN_LIFETIME: Duration = Duration::hours(48);

pub struct HmacCheckoutCredentialCodec {
    secret: Vec<u8>,
}

impl HmacCheckoutCredentialCodec {
    pub fn new(secret: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let secret = secret.into();
        if secret.len() < 32 {
            anyhow::bail!("checkout token secret must contain at least 32 bytes");
        }
        Ok(Self { secret })
    }

    fn signature(
        &self,
        order_id: Uuid,
        store_id: Uuid,
        channel_id: Uuid,
        expires_at: i64,
    ) -> Result<Vec<u8>, ApplicationError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(signing_input(order_id, store_id, channel_id, expires_at).as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

impl CheckoutCredentialCodec for HmacCheckoutCredentialCodec {
    fn issue(
        &self,
        actor: &MachineActor,
        order_id: OrderId,
        now: OffsetDateTime,
    ) -> Result<SecretString, ApplicationError> {
        let channel_id = actor.channel_id.ok_or(ApplicationError::Unauthorized)?;
        let expires_at = now
            .checked_add(TOKEN_LIFETIME)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!("checkout token expiry overflow"))
            })?
            .unix_timestamp();
        let signature = self.signature(
            order_id.as_uuid(),
            actor.store_id.as_uuid(),
            channel_id.as_uuid(),
            expires_at,
        )?;
        Ok(SecretString::from(format!(
            "{TOKEN_PREFIX}.{}.{}.{signature}",
            order_id.as_uuid().simple(),
            expires_at,
            signature = URL_SAFE_NO_PAD.encode(signature),
        )))
    }

    fn verify(
        &self,
        actor: &MachineActor,
        credential: &SecretString,
        now: OffsetDateTime,
    ) -> Result<CheckoutActor, ApplicationError> {
        let parts = credential.expose_secret().split('.').collect::<Vec<_>>();
        if parts.len() != 4 || parts[0] != TOKEN_PREFIX {
            return Err(ApplicationError::Unauthorized);
        }
        let order_id = Uuid::parse_str(parts[1]).map_err(|_| ApplicationError::Unauthorized)?;
        let expires_at = parts[2]
            .parse::<i64>()
            .map_err(|_| ApplicationError::Unauthorized)?;
        if now.unix_timestamp() >= expires_at {
            return Err(ApplicationError::Unauthorized);
        }
        let presented = URL_SAFE_NO_PAD
            .decode(parts[3])
            .map_err(|_| ApplicationError::Unauthorized)?;
        let channel_id = actor.channel_id.ok_or(ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(
            signing_input(
                order_id,
                actor.store_id.as_uuid(),
                channel_id.as_uuid(),
                expires_at,
            )
            .as_bytes(),
        );
        mac.verify_slice(&presented)
            .map_err(|_| ApplicationError::Unauthorized)?;
        Ok(CheckoutActor::new(
            actor.clone(),
            OrderId::from_uuid(order_id),
        ))
    }
}

fn signing_input(order_id: Uuid, store_id: Uuid, channel_id: Uuid, expires_at: i64) -> String {
    format!("{TOKEN_PREFIX}:{order_id}:{store_id}:{channel_id}:{expires_at}")
}

#[cfg(test)]
mod tests {
    use chaos_domain::store::{PublishableKeyId, SalesChannelId, StoreId};
    use secrecy::{ExposeSecret, SecretString};
    use time::{Duration, OffsetDateTime};

    use crate::contracts::{CheckoutCredentialCodec, MachineActor};

    use super::HmacCheckoutCredentialCodec;

    fn new_actor() -> MachineActor {
        MachineActor {
            publishable_key_id: PublishableKeyId::new(),
            store_id: StoreId::new(),
            channel_id: Some(SalesChannelId::new()),
        }
    }

    #[test]
    fn checkout_capability_is_order_channel_and_time_bound() {
        let codec = HmacCheckoutCredentialCodec::new([9_u8; 32]).unwrap();
        let actor = new_actor();
        let order_id = chaos_domain::sales::OrderId::new();
        let now = OffsetDateTime::UNIX_EPOCH;
        let token = codec.issue(&actor, order_id, now).unwrap();

        assert_eq!(
            codec.verify(&actor, &token, now).unwrap().order_id(),
            order_id
        );
        assert!(
            codec
                .verify(&actor, &token, now + Duration::hours(24))
                .is_ok()
        );
        assert!(
            codec
                .verify(&actor, &token, now + Duration::hours(48))
                .is_err()
        );
        assert!(codec.verify(&new_actor(), &token, now).is_err());

        let modified = SecretString::from(format!("{}x", token.expose_secret()));
        assert!(codec.verify(&actor, &modified, now).is_err());
    }
}
