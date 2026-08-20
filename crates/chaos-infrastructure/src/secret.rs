use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        MetaDeliveryError, PaymentSecretResolver, ProviderSecretKind, ProviderSecretWriter,
        ShippingSecretResolver,
    },
};
use chaos_domain::{
    fulfillment::ShippingSecretReference, identity::UserId, payments::PaymentSecretReference,
    store::StoreId,
};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};

use crate::config::SecretKey;

const ENCRYPTED_PREFIX: &str = "enc://";
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct DynamicSecretResolver {
    store: Arc<EncryptedProviderSecretStore>,
}

impl DynamicSecretResolver {
    pub fn new(key: &SecretKey) -> Self {
        Self {
            store: Arc::new(EncryptedProviderSecretStore::new(key)),
        }
    }

    async fn resolve_reference(
        &self,
        reference: &str,
        environment_prefix: &str,
    ) -> Result<SecretString, SecretUnavailable> {
        if let Some(variable) = reference.strip_prefix("env://") {
            if !variable.starts_with(environment_prefix)
                || !variable
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(SecretUnavailable);
            }
            return std::env::var(variable)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(SecretString::from)
                .ok_or(SecretUnavailable);
        }
        self.store.decrypt(reference)
    }

    pub async fn resolve_analytics(
        &self,
        reference: &str,
    ) -> Result<SecretString, MetaDeliveryError> {
        self.resolve_reference(reference, "CHAOS_ANALYTICS_SECRET_")
            .await
            .map_err(|_| MetaDeliveryError {
                retryable: true,
                message: "Meta credentials are unavailable".into(),
            })
    }
}

#[async_trait]
impl PaymentSecretResolver for DynamicSecretResolver {
    async fn resolve(
        &self,
        reference: &PaymentSecretReference,
    ) -> Result<SecretString, ApplicationError> {
        self.resolve_reference(reference.expose_reference(), "CHAOS_PAYMENT_SECRET_")
            .await
            .map_err(|_| ApplicationError::Unavailable {
                service: "payment_secret_manager",
                source: anyhow::anyhow!("Payment Provider credentials are unavailable"),
            })
    }
}

#[async_trait]
impl ShippingSecretResolver for DynamicSecretResolver {
    async fn resolve(
        &self,
        reference: &ShippingSecretReference,
    ) -> Result<SecretString, ApplicationError> {
        self.resolve_reference(reference.expose_reference(), "CHAOS_SHIPPING_SECRET_")
            .await
            .map_err(|_| ApplicationError::Conflict {
                code: "shipping_provider_credentials_unavailable",
                message: "The shipping provider credentials are unavailable",
            })
    }
}

#[async_trait]
impl ProviderSecretWriter for DynamicSecretResolver {
    async fn create(
        &self,
        _store_id: StoreId,
        _created_by: UserId,
        _kind: ProviderSecretKind,
        value: &SecretString,
    ) -> Result<String, ApplicationError> {
        self.store
            .encrypt(value)
            .map_err(|_| secret_manager_unavailable())
    }
}

struct EncryptedProviderSecretStore {
    cipher: Aes256Gcm,
}

impl EncryptedProviderSecretStore {
    fn new(key: &SecretKey) -> Self {
        Self {
            cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key.expose_bytes())),
        }
    }

    fn encrypt(&self, value: &SecretString) -> Result<String, SecretUnavailable> {
        let mut nonce_bytes = [0_u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, value.expose_secret().as_bytes())
            .map_err(|_| SecretUnavailable)?;
        let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);
        Ok(format!(
            "{ENCRYPTED_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(payload)
        ))
    }

    fn decrypt(&self, reference: &str) -> Result<SecretString, SecretUnavailable> {
        let encoded = reference
            .strip_prefix(ENCRYPTED_PREFIX)
            .ok_or(SecretUnavailable)?;
        let payload = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SecretUnavailable)?;
        if payload.len() <= NONCE_LEN {
            return Err(SecretUnavailable);
        }
        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).map_err(|_| SecretUnavailable)?;
        let plaintext = self
            .cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| SecretUnavailable)?;
        String::from_utf8(plaintext)
            .map(SecretString::from)
            .map_err(|_| SecretUnavailable)
    }
}

fn secret_manager_unavailable() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "provider_secret_manager",
        source: anyhow::anyhow!("Provider secret manager is unavailable"),
    }
}

#[derive(Debug)]
struct SecretUnavailable;

#[cfg(test)]
mod tests {
    use super::*;

    fn store(seed: u8) -> EncryptedProviderSecretStore {
        EncryptedProviderSecretStore::new(&SecretKey::from_raw([seed; 32]))
    }

    #[test]
    fn round_trips_encrypted_values() {
        let store = store(1);
        let reference = store
            .encrypt(&SecretString::from("sk_live_secret"))
            .unwrap();

        assert!(reference.starts_with("enc://"));
        let value = store.decrypt(&reference).unwrap();
        assert_eq!(value.expose_secret(), "sk_live_secret");
    }

    #[test]
    fn distinct_encryptions_of_the_same_value_produce_distinct_references() {
        let store = store(1);
        let value = SecretString::from("sk_live_secret");

        let first = store.encrypt(&value).unwrap();
        let second = store.encrypt(&value).unwrap();

        assert_ne!(first, second);
        assert_eq!(
            store.decrypt(&first).unwrap().expose_secret(),
            "sk_live_secret"
        );
        assert_eq!(
            store.decrypt(&second).unwrap().expose_secret(),
            "sk_live_secret"
        );
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let store = store(1);
        let reference = store
            .encrypt(&SecretString::from("sk_live_secret"))
            .unwrap();
        let encoded = reference.strip_prefix("enc://").unwrap();
        let mut payload = URL_SAFE_NO_PAD.decode(encoded).unwrap();
        let last = payload.len() - 1;
        payload[last] ^= 0xFF;
        let tampered = format!("enc://{}", URL_SAFE_NO_PAD.encode(payload));

        assert!(store.decrypt(&tampered).is_err());
    }

    #[test]
    fn rejects_decryption_with_the_wrong_key() {
        let encrypting_store = store(1);
        let decrypting_store = store(2);
        let reference = encrypting_store
            .encrypt(&SecretString::from("sk_live_secret"))
            .unwrap();

        assert!(decrypting_store.decrypt(&reference).is_err());
    }

    #[test]
    fn rejects_malformed_references() {
        let store = store(1);

        assert!(
            store
                .decrypt("vault://secret/chaos/stores/store-1/stripe")
                .is_err()
        );
        assert!(store.decrypt("enc://not-valid-base64!!!").is_err());
        assert!(store.decrypt("enc://").is_err());
        assert!(
            store
                .decrypt(&format!("enc://{}", URL_SAFE_NO_PAD.encode([0_u8; 4])))
                .is_err()
        );
    }

    #[tokio::test]
    async fn resolves_environment_references_for_payment_secrets() {
        let resolver = DynamicSecretResolver::new(&SecretKey::from_raw([1; 32]));
        // SAFETY: test-only, single-threaded env var scoped to this test.
        unsafe {
            std::env::set_var("CHAOS_PAYMENT_SECRET_TEST", "sk_live_from_env");
        }
        let reference = PaymentSecretReference::new(
            "credential_secret_reference",
            "env://CHAOS_PAYMENT_SECRET_TEST",
        )
        .unwrap();

        let value = PaymentSecretResolver::resolve(&resolver, &reference)
            .await
            .unwrap();

        assert_eq!(value.expose_secret(), "sk_live_from_env");
        unsafe {
            std::env::remove_var("CHAOS_PAYMENT_SECRET_TEST");
        }
    }

    #[tokio::test]
    async fn creates_and_resolves_an_encrypted_provider_secret() {
        let resolver = DynamicSecretResolver::new(&SecretKey::from_raw([7; 32]));
        let store_id = StoreId::new();
        let created_by = UserId::new();

        let reference = ProviderSecretWriter::create(
            &resolver,
            store_id,
            created_by,
            ProviderSecretKind::PaymentWebhook,
            &SecretString::from("whsec_test"),
        )
        .await
        .unwrap();

        assert!(reference.starts_with("enc://"));
        let parsed = PaymentSecretReference::new("credential_secret_reference", reference).unwrap();
        let value = PaymentSecretResolver::resolve(&resolver, &parsed)
            .await
            .unwrap();
        assert_eq!(value.expose_secret(), "whsec_test");
    }
}
