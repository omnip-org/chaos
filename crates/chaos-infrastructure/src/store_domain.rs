use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        StoreDomainOwnershipVerifier, StoreDomainVerificationMaterial,
        StoreDomainVerificationMaterialGenerator,
    },
};
use chaos_domain::merchant::StoreHostname;
use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::RData;
use rand::RngCore;
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Default)]
pub struct SecureStoreDomainVerificationMaterialGenerator;

impl StoreDomainVerificationMaterialGenerator for SecureStoreDomainVerificationMaterialGenerator {
    fn generate(&self) -> StoreDomainVerificationMaterial {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        StoreDomainVerificationMaterial {
            token_digest: Sha256::digest(token.as_bytes()).into(),
            plaintext_token: SecretString::from(token),
        }
    }
}

pub struct DnsStoreDomainOwnershipVerifier {
    resolver: TokioResolver,
}

impl DnsStoreDomainOwnershipVerifier {
    pub fn from_system_configuration(timeout: Duration) -> anyhow::Result<Self> {
        let mut builder = TokioResolver::builder_tokio()?;
        builder.options_mut().timeout = timeout;
        builder.options_mut().attempts = 1;
        Ok(Self {
            resolver: builder.build()?,
        })
    }
}

#[async_trait]
impl StoreDomainOwnershipVerifier for DnsStoreDomainOwnershipVerifier {
    async fn verify(
        &self,
        hostname: &StoreHostname,
        expected_token_digest: &[u8; 32],
    ) -> Result<bool, ApplicationError> {
        let Ok(lookup) = self
            .resolver
            .txt_lookup(hostname.verification_record_name())
            .await
        else {
            return Ok(false);
        };
        Ok(lookup.answers().iter().any(|record| {
            let RData::TXT(txt) = &record.data else {
                return false;
            };
            let value = txt
                .txt_data
                .iter()
                .flat_map(|part| part.iter().copied())
                .collect::<Vec<_>>();
            let Some(token) = value.strip_prefix(b"chaos-domain-verification=") else {
                return false;
            };
            <[u8; 32]>::from(Sha256::digest(token)) == *expected_token_digest
        }))
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn verification_material_is_random_and_digest_bound() {
        let generator = SecureStoreDomainVerificationMaterialGenerator;
        let first = generator.generate();
        let second = generator.generate();
        assert_ne!(
            first.plaintext_token.expose_secret(),
            second.plaintext_token.expose_secret()
        );
        assert_eq!(
            first.token_digest,
            <[u8; 32]>::from(Sha256::digest(
                first.plaintext_token.expose_secret().as_bytes()
            ))
        );
    }
}
