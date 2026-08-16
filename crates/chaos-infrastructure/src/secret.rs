use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AnalyticsDestinationSecretResolver, PaymentSecretResolver, ProviderSecretKind,
        ProviderSecretWriter, ShippingSecretResolver,
    },
};
use chaos_domain::{
    analytics::AnalyticsDestinationSecretReference,
    fulfillment::ShippingSecretReference,
    identity::UserId,
    merchant::{MerchantAccountId, StoreId},
    payments::PaymentSecretReference,
};
use reqwest::{
    Client, StatusCode, Url,
    header::{HeaderMap, HeaderValue},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::VaultSettings;

const VAULT_TOKEN_HEADER: &str = "x-vault-token";
const VAULT_NAMESPACE_HEADER: &str = "x-vault-namespace";

#[derive(Clone)]
pub struct DynamicSecretResolver {
    vault: Option<Arc<VaultKvV2SecretStore>>,
}

impl DynamicSecretResolver {
    pub fn new(settings: Option<&VaultSettings>, timeout: Duration) -> anyhow::Result<Self> {
        let vault = settings
            .map(|settings| VaultKvV2SecretStore::new(settings, timeout).map(Arc::new))
            .transpose()?;
        Ok(Self { vault })
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
        let vault = self.vault.as_ref().ok_or(SecretUnavailable)?;
        vault.resolve(reference).await
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
impl AnalyticsDestinationSecretResolver for DynamicSecretResolver {
    async fn resolve(
        &self,
        reference: &AnalyticsDestinationSecretReference,
    ) -> Result<SecretString, chaos_application::ports::AnalyticsDestinationError> {
        self.resolve_reference(reference.expose_reference(), "CHAOS_ANALYTICS_SECRET_")
            .await
            .map_err(|_| chaos_application::ports::AnalyticsDestinationError {
                retryable: true,
                message: "Analytics destination credentials are unavailable".into(),
            })
    }
}

#[async_trait]
impl ProviderSecretWriter for DynamicSecretResolver {
    async fn create(
        &self,
        merchant_account_id: MerchantAccountId,
        store_id: StoreId,
        created_by: UserId,
        kind: ProviderSecretKind,
        value: &SecretString,
    ) -> Result<String, ApplicationError> {
        let vault = self.vault.as_ref().ok_or_else(secret_manager_unavailable)?;
        vault
            .create(merchant_account_id, store_id, created_by, kind, value)
            .await
            .map_err(|_| secret_manager_unavailable())
    }
}

struct VaultKvV2SecretStore {
    client: Client,
    address: Url,
    token: SecretString,
    namespace: Option<String>,
    kv_v2_mount: String,
}

impl VaultKvV2SecretStore {
    fn new(settings: &VaultSettings, timeout: Duration) -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = Client::builder().timeout(timeout);
        if let Some(path) = &settings.ca_certificate_path {
            let certificate = std::fs::read(path)
                .map_err(|error| anyhow::anyhow!("failed to read VAULT_CA_CERT: {error}"))?;
            client = client.add_root_certificate(reqwest::Certificate::from_pem(&certificate)?);
        }
        Ok(Self {
            client: client.build()?,
            address: settings.address.clone(),
            token: settings.token.clone(),
            namespace: settings.namespace.clone(),
            kv_v2_mount: settings.kv_v2_mount.clone(),
        })
    }

    async fn resolve(&self, reference: &str) -> Result<SecretString, SecretUnavailable> {
        let location = VaultLocation::parse(reference)?;
        let endpoint = self
            .address
            .join(&format!("/v1/{}/data/{}", location.mount, location.path))
            .map_err(|_| SecretUnavailable)?;
        let response = self
            .client
            .get(endpoint)
            .headers(self.headers()?)
            .send()
            .await
            .map_err(|_| SecretUnavailable)?;
        if response.status() != StatusCode::OK {
            return Err(SecretUnavailable);
        }
        let response: VaultReadResponse = response.json().await.map_err(|_| SecretUnavailable)?;
        response
            .data
            .data
            .get("value")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(|value| SecretString::from(value.to_owned()))
            .ok_or(SecretUnavailable)
    }

    async fn create(
        &self,
        merchant_account_id: MerchantAccountId,
        store_id: StoreId,
        created_by: UserId,
        kind: ProviderSecretKind,
        value: &SecretString,
    ) -> Result<String, SecretUnavailable> {
        if !valid_segment(&self.kv_v2_mount) {
            return Err(SecretUnavailable);
        }
        let path = format!(
            "chaos/stores/{}/{}/{}/{}",
            merchant_account_id.as_uuid(),
            store_id.as_uuid(),
            kind.as_path_segment(),
            uuid::Uuid::now_v7()
        );
        let endpoint = self
            .address
            .join(&format!("/v1/{}/data/{path}", self.kv_v2_mount))
            .map_err(|_| SecretUnavailable)?;
        let response = self
            .client
            .post(endpoint)
            .headers(self.headers()?)
            .json(&VaultWriteRequest {
                options: VaultWriteOptions { cas: 0 },
                data: VaultWriteData {
                    value: value.expose_secret(),
                    merchant_account_id: merchant_account_id.as_uuid().to_string(),
                    store_id: store_id.as_uuid().to_string(),
                    created_by_user_id: created_by.as_uuid().to_string(),
                    kind: kind.as_path_segment(),
                },
            })
            .send()
            .await
            .map_err(|_| SecretUnavailable)?;
        if !response.status().is_success() {
            return Err(SecretUnavailable);
        }
        Ok(format!("vault://{}/{path}", self.kv_v2_mount))
    }

    fn headers(&self) -> Result<HeaderMap, SecretUnavailable> {
        let mut headers = HeaderMap::new();
        headers.insert(
            VAULT_TOKEN_HEADER,
            HeaderValue::from_str(self.token.expose_secret()).map_err(|_| SecretUnavailable)?,
        );
        if let Some(namespace) = &self.namespace {
            headers.insert(
                VAULT_NAMESPACE_HEADER,
                HeaderValue::from_str(namespace).map_err(|_| SecretUnavailable)?,
            );
        }
        Ok(headers)
    }
}

struct VaultLocation<'a> {
    mount: &'a str,
    path: &'a str,
}

impl<'a> VaultLocation<'a> {
    fn parse(reference: &'a str) -> Result<Self, SecretUnavailable> {
        let location = reference
            .strip_prefix("vault://")
            .ok_or(SecretUnavailable)?;
        let (mount, path) = location.split_once('/').ok_or(SecretUnavailable)?;
        if !valid_segment(mount)
            || path.is_empty()
            || path.len() > 220
            || !path.split('/').all(valid_segment)
        {
            return Err(SecretUnavailable);
        }
        Ok(Self { mount, path })
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[derive(Deserialize)]
struct VaultReadResponse {
    data: VaultReadData,
}

#[derive(Deserialize)]
struct VaultReadData {
    data: HashMap<String, Value>,
}

#[derive(Serialize)]
struct VaultWriteRequest<'a> {
    options: VaultWriteOptions,
    data: VaultWriteData<'a>,
}

#[derive(Serialize)]
struct VaultWriteOptions {
    cas: u8,
}

#[derive(Serialize)]
struct VaultWriteData<'a> {
    value: &'a str,
    merchant_account_id: String,
    store_id: String,
    created_by_user_id: String,
    kind: &'static str,
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
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Json, Router,
        extract::{OriginalUri, State},
        http::HeaderMap,
        routing::{get, post},
    };
    use serde_json::json;

    use super::*;

    type CapturedVaultWrite = Arc<Mutex<Option<(String, Value)>>>;

    #[test]
    fn accepts_a_bounded_vault_kv_v2_location() {
        let location = VaultLocation::parse("vault://secret/chaos/stores/store-1/stripe").unwrap();
        assert_eq!(location.mount, "secret");
        assert_eq!(location.path, "chaos/stores/store-1/stripe");
    }

    #[test]
    fn rejects_traversal_and_non_vault_references() {
        assert!(VaultLocation::parse("vault://secret/../stripe").is_err());
        assert!(VaultLocation::parse("env://CHAOS_PAYMENT_SECRET_TEST").is_err());
    }

    #[tokio::test]
    async fn reads_the_current_value_from_vault_kv_v2() {
        let reads = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/v1/secret/data/chaos/stores/store-1/stripe",
            get({
                let reads = reads.clone();
                move |headers: HeaderMap| {
                    let reads = reads.clone();
                    async move {
                        assert_eq!(headers[VAULT_TOKEN_HEADER], "test-token");
                        let value = match reads.fetch_add(1, Ordering::SeqCst) {
                            0 => "first-secret",
                            _ => "rotated-secret",
                        };
                        Json(json!({ "data": { "data": { "value": value } } }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: Url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = VaultKvV2SecretStore::new(
            &VaultSettings {
                address,
                token: SecretString::from("test-token"),
                namespace: None,
                ca_certificate_path: None,
                kv_v2_mount: "secret".into(),
            },
            Duration::from_secs(1),
        )
        .unwrap();

        let first = store
            .resolve("vault://secret/chaos/stores/store-1/stripe")
            .await
            .unwrap();
        let second = store
            .resolve("vault://secret/chaos/stores/store-1/stripe")
            .await
            .unwrap();

        assert_eq!(first.expose_secret(), "first-secret");
        assert_eq!(second.expose_secret(), "rotated-secret");
    }

    #[tokio::test]
    async fn creates_a_new_store_scoped_secret_with_cas_zero() {
        let request: CapturedVaultWrite = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route(
                "/{*path}",
                post(
                    |State(request): State<CapturedVaultWrite>,
                     OriginalUri(uri): OriginalUri,
                     Json(body): Json<Value>| async move {
                        *request.lock().unwrap() = Some((uri.to_string(), body));
                        Json(json!({ "data": { "version": 1 } }))
                    },
                ),
            )
            .with_state(request.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: Url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let settings = VaultSettings {
            address,
            token: SecretString::from("test-token"),
            namespace: None,
            ca_certificate_path: None,
            kv_v2_mount: "secret".into(),
        };
        let resolver = DynamicSecretResolver::new(Some(&settings), Duration::from_secs(1)).unwrap();
        let merchant_account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let created_by = UserId::new();

        let reference = ProviderSecretWriter::create(
            &resolver,
            merchant_account_id,
            store_id,
            created_by,
            ProviderSecretKind::PaymentWebhook,
            &SecretString::from("whsec_test"),
        )
        .await
        .unwrap();

        assert!(reference.starts_with(&format!(
            "vault://secret/chaos/stores/{}/{}/payment-webhook/",
            merchant_account_id.as_uuid(),
            store_id.as_uuid()
        )));
        let (path, body) = request.lock().unwrap().take().unwrap();
        assert!(path.starts_with("/v1/secret/data/chaos/stores/"));
        assert_eq!(body["options"]["cas"], 0);
        assert_eq!(body["data"]["value"], "whsec_test");
        assert_eq!(
            body["data"]["created_by_user_id"],
            created_by.as_uuid().to_string()
        );
    }
}
