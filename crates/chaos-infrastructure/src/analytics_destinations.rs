use std::{str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use chaos_application::ports::{
    AnalyticsDestination, AnalyticsDestinationError, AnalyticsDestinationSecretResolver,
    AnalyticsExportCommand, AnalyticsExportReceipt,
};
use chaos_domain::analytics::{AnalyticsDestinationProvider, AnalyticsDestinationSecretReference};
use reqwest::{Client, StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value, json};
use sha2::{Digest, Sha256};

pub struct EnvironmentAnalyticsDestinationSecretResolver;

#[async_trait]
impl AnalyticsDestinationSecretResolver for EnvironmentAnalyticsDestinationSecretResolver {
    async fn resolve(
        &self,
        reference: &AnalyticsDestinationSecretReference,
    ) -> Result<SecretString, AnalyticsDestinationError> {
        let variable = reference
            .expose_reference()
            .strip_prefix("env://")
            .ok_or_else(secret_unavailable)?;
        std::env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(SecretString::from)
            .ok_or_else(secret_unavailable)
    }
}

pub struct MetaConversionsDestination {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn AnalyticsDestinationSecretResolver>,
}

pub struct Ga4MeasurementDestination {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn AnalyticsDestinationSecretResolver>,
}

impl MetaConversionsDestination {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn AnalyticsDestinationSecretResolver>,
    ) -> anyhow::Result<Self> {
        validate_base_url(&api_base_url, "Meta Conversions API")?;
        Ok(Self {
            client: client(timeout)?,
            api_base_url,
            secrets,
        })
    }
}

impl Ga4MeasurementDestination {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn AnalyticsDestinationSecretResolver>,
    ) -> anyhow::Result<Self> {
        validate_base_url(&api_base_url, "GA4 Measurement Protocol")?;
        Ok(Self {
            client: client(timeout)?,
            api_base_url,
            secrets,
        })
    }
}

#[async_trait]
impl AnalyticsDestination for MetaConversionsDestination {
    fn provider(&self) -> AnalyticsDestinationProvider {
        AnalyticsDestinationProvider::MetaCapi
    }

    async fn send(
        &self,
        command: &AnalyticsExportCommand,
    ) -> Result<AnalyticsExportReceipt, AnalyticsDestinationError> {
        let token = self
            .secrets
            .resolve(&command.credential_secret_reference)
            .await?;
        let source_base = command
            .event_source_base_url
            .as_deref()
            .ok_or_else(invalid_command)?;
        let source_url = Url::parse(source_base)
            .and_then(|base| base.join(&format!("orders/{}", command.order_id)))
            .map_err(|_| invalid_command())?;
        let endpoint = self
            .api_base_url
            .join(&format!(
                "{}/events",
                command.external_destination_reference
            ))
            .map_err(|_| invalid_command())?;
        let payload = MetaRequest {
            data: [MetaEvent {
                event_name: meta_event_name(&command.commerce_event_name),
                event_time: command.occurred_at.unix_timestamp(),
                event_id: command.order_id.to_string(),
                action_source: "website",
                event_source_url: source_url.as_str(),
                user_data: MetaUserData {
                    external_id: [sha256_hex(command.anonymous_id.to_string().as_bytes())],
                },
                custom_data: custom_data(command)?,
            }],
        };
        let response = self
            .client
            .post(endpoint)
            .query(&[("access_token", token.expose_secret())])
            .json(&payload)
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status));
        }
        let receipt: MetaResponse = response.json().await.map_err(|_| invalid_response())?;
        if receipt.events_received < 1 || receipt.fbtrace_id.is_empty() {
            return Err(invalid_response());
        }
        Ok(AnalyticsExportReceipt {
            provider_reference: receipt.fbtrace_id,
        })
    }
}

#[async_trait]
impl AnalyticsDestination for Ga4MeasurementDestination {
    fn provider(&self) -> AnalyticsDestinationProvider {
        AnalyticsDestinationProvider::Ga4
    }

    async fn send(
        &self,
        command: &AnalyticsExportCommand,
    ) -> Result<AnalyticsExportReceipt, AnalyticsDestinationError> {
        let secret = self
            .secrets
            .resolve(&command.credential_secret_reference)
            .await?;
        let endpoint = self
            .api_base_url
            .join("mp/collect")
            .map_err(|_| invalid_command())?;
        let event = ga4_event(command)?;
        let payload = Ga4Request {
            client_id: ga4_client_id(command.anonymous_id),
            timestamp_micros: i64::try_from(command.occurred_at.unix_timestamp_nanos() / 1_000)
                .map_err(|_| invalid_command())?,
            events: [event],
        };
        let response = self
            .client
            .post(endpoint)
            .query(&[
                (
                    "measurement_id",
                    command.external_destination_reference.as_str(),
                ),
                ("api_secret", secret.expose_secret()),
            ])
            .json(&payload)
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status));
        }
        Ok(AnalyticsExportReceipt {
            provider_reference: command.fact_id.to_string(),
        })
    }
}

#[derive(Serialize)]
struct MetaRequest<'a> {
    data: [MetaEvent<'a>; 1],
}

#[derive(Serialize)]
struct MetaEvent<'a> {
    event_name: &'static str,
    event_time: i64,
    event_id: String,
    action_source: &'static str,
    event_source_url: &'a str,
    user_data: MetaUserData,
    custom_data: Value,
}

#[derive(Serialize)]
struct MetaUserData {
    external_id: [String; 1],
}

#[derive(Deserialize)]
struct MetaResponse {
    events_received: u32,
    fbtrace_id: String,
}

#[derive(Serialize)]
struct Ga4Request {
    client_id: String,
    timestamp_micros: i64,
    events: [Ga4Event; 1],
}

#[derive(Serialize)]
struct Ga4Event {
    name: &'static str,
    params: Value,
}

fn custom_data(command: &AnalyticsExportCommand) -> Result<Value, AnalyticsDestinationError> {
    let contents = command
        .items
        .iter()
        .map(|item| {
            Ok(json!({
                "id": item.product_variant_id,
                "title": item.item_name,
                "quantity": item.quantity,
                "item_price": major_number(item.unit_price_amount_minor, &command.currency)?,
            }))
        })
        .collect::<Result<Vec<_>, AnalyticsDestinationError>>()?;
    Ok(json!({
        "currency": command.currency,
        "value": command.amount_minor.map(|amount| major_number(amount, &command.currency)).transpose()?,
        "order_id": command.order_id,
        "content_type": "product",
        "contents": contents,
    }))
}

fn ga4_event(command: &AnalyticsExportCommand) -> Result<Ga4Event, AnalyticsDestinationError> {
    let items = command
        .items
        .iter()
        .map(|item| {
            Ok(json!({
                "item_id": item.product_variant_id,
                "item_name": item.item_name,
                "price": major_number(item.unit_price_amount_minor, &command.currency)?,
                "quantity": item.quantity,
            }))
        })
        .collect::<Result<Vec<_>, AnalyticsDestinationError>>()?;
    let (name, mut params) = match command.commerce_event_name.as_str() {
        "payment_captured" => (
            "purchase",
            json!({
                "transaction_id": command.order_id,
                "currency": command.currency,
                "value": command.amount_minor.map(|amount| major_number(amount, &command.currency)).transpose()?,
                "shipping": major_number(command.shipping_amount_minor, &command.currency)?,
                "tax": major_number(command.tax_amount_minor, &command.currency)?,
                "items": items,
            }),
        ),
        "refund_succeeded" => (
            "refund",
            json!({
                "transaction_id": command.order_id,
                "currency": command.currency,
                "value": command.amount_minor.map(|amount| major_number(amount, &command.currency)).transpose()?,
                "items": items,
            }),
        ),
        other => (
            ga4_custom_event_name(other),
            json!({"transaction_id": command.order_id}),
        ),
    };
    params["chaos_fact_id"] = json!(command.fact_id);
    Ok(Ga4Event { name, params })
}

fn meta_event_name(name: &str) -> &'static str {
    match name {
        "payment_captured" => "Purchase",
        "refund_succeeded" => "Refund",
        "order_created" => "OrderCreated",
        "fulfillment_shipped" => "FulfillmentShipped",
        "return_completed" => "ReturnCompleted",
        _ => "CommerceEvent",
    }
}

fn ga4_custom_event_name(name: &str) -> &'static str {
    match name {
        "order_created" => "order_created",
        "fulfillment_shipped" => "fulfillment_shipped",
        "return_completed" => "return_completed",
        _ => "commerce_event",
    }
}

fn ga4_client_id(id: uuid::Uuid) -> String {
    let value = id.as_u128();
    let high = ((value >> 64) as u64).max(1);
    let low = (value as u64).max(1);
    format!("{high}.{low}")
}

fn major_number(amount_minor: u64, currency: &str) -> Result<Number, AnalyticsDestinationError> {
    let exponent = match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    };
    let divisor = 10_u64.pow(exponent);
    let value = if exponent == 0 {
        amount_minor.to_string()
    } else {
        format!(
            "{}.{:0width$}",
            amount_minor / divisor,
            amount_minor % divisor,
            width = exponent as usize
        )
    };
    Number::from_str(&value).map_err(|_| invalid_command())
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_base_url(url: &Url, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        url.scheme() == "https" || url.host_str().is_some_and(is_loopback),
        "{name} base URL must use HTTPS outside loopback tests"
    );
    Ok(())
}

fn client(timeout: Duration) -> anyhow::Result<Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(Client::builder().timeout(timeout).build()?)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn secret_unavailable() -> AnalyticsDestinationError {
    AnalyticsDestinationError {
        retryable: true,
        message: "Analytics destination credentials are unavailable".into(),
    }
}

fn invalid_command() -> AnalyticsDestinationError {
    AnalyticsDestinationError {
        retryable: false,
        message: "Analytics destination command is invalid".into(),
    }
}

fn invalid_response() -> AnalyticsDestinationError {
    AnalyticsDestinationError {
        retryable: false,
        message: "Analytics destination returned an invalid response".into(),
    }
}

fn network_error(_error: reqwest::Error) -> AnalyticsDestinationError {
    AnalyticsDestinationError {
        retryable: true,
        message: "Analytics destination request failed".into(),
    }
}

fn http_error(status: StatusCode) -> AnalyticsDestinationError {
    AnalyticsDestinationError {
        retryable: status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT,
        message: format!("Analytics destination returned HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::{
        Json, Router,
        extract::{OriginalUri, State},
        routing::post,
    };
    use chaos_application::ports::AnalyticsExportItem;
    use time::OffsetDateTime;
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct Requests(Mutex<Vec<(String, Value)>>);

    struct StaticSecrets;

    #[async_trait]
    impl AnalyticsDestinationSecretResolver for StaticSecrets {
        async fn resolve(
            &self,
            _reference: &AnalyticsDestinationSecretReference,
        ) -> Result<SecretString, AnalyticsDestinationError> {
            Ok(SecretString::from("test-secret"))
        }
    }

    async fn mock(
        State(requests): State<Arc<Requests>>,
        OriginalUri(uri): OriginalUri,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        requests
            .0
            .lock()
            .expect("request lock")
            .push((uri.to_string(), body));
        Json(json!({"events_received": 1, "fbtrace_id": "trace-123"}))
    }

    async fn destinations() -> (
        MetaConversionsDestination,
        Ga4MeasurementDestination,
        Arc<Requests>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::new(Requests::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let app = Router::new()
            .route("/{*path}", post(mock))
            .with_state(requests.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let base_url = Url::parse(&format!("http://127.0.0.1:{}/", address.port())).unwrap();
        let secrets = Arc::new(StaticSecrets);
        (
            MetaConversionsDestination::new(
                base_url.clone(),
                Duration::from_secs(1),
                secrets.clone(),
            )
            .unwrap(),
            Ga4MeasurementDestination::new(base_url, Duration::from_secs(1), secrets).unwrap(),
            requests,
            server,
        )
    }

    fn command(provider: AnalyticsDestinationProvider) -> AnalyticsExportCommand {
        AnalyticsExportCommand {
            provider,
            external_destination_reference: match provider {
                AnalyticsDestinationProvider::MetaCapi => "123456789".into(),
                AnalyticsDestinationProvider::Ga4 => "G-ABC123".into(),
            },
            event_source_base_url: (provider == AnalyticsDestinationProvider::MetaCapi)
                .then(|| "https://store.example/".into()),
            credential_secret_reference: AnalyticsDestinationSecretReference::new(
                "env://CHAOS_ANALYTICS_SECRET_TEST",
            )
            .unwrap(),
            commerce_event_name: "payment_captured".into(),
            fact_id: Uuid::parse_str("0198b202-8e18-7a21-8f31-e69167390310").unwrap(),
            order_id: Uuid::parse_str("0198b202-8e18-7a21-8f31-e69167390311").unwrap(),
            anonymous_id: Uuid::parse_str("0198b202-8e18-7a21-8f31-e69167390312").unwrap(),
            occurred_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            amount_minor: Some(1234),
            currency: "USD".into(),
            shipping_amount_minor: 200,
            tax_amount_minor: 100,
            items: vec![AnalyticsExportItem {
                product_variant_id: Uuid::parse_str("0198b202-8e18-7a21-8f31-e69167390313")
                    .unwrap(),
                item_name: "T-Shirt".into(),
                quantity: 2,
                unit_price_amount_minor: 617,
            }],
        }
    }

    #[tokio::test]
    async fn adapters_map_purchase_and_share_order_deduplication_identity() {
        let (meta, ga4, requests, server) = destinations().await;
        meta.send(&command(AnalyticsDestinationProvider::MetaCapi))
            .await
            .unwrap();
        ga4.send(&command(AnalyticsDestinationProvider::Ga4))
            .await
            .unwrap();

        let requests = requests.0.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .0
                .starts_with("/123456789/events?access_token=test-secret")
        );
        assert_eq!(requests[0].1["data"][0]["event_name"], "Purchase");
        assert_eq!(
            requests[0].1["data"][0]["event_id"],
            "0198b202-8e18-7a21-8f31-e69167390311"
        );
        assert_ne!(
            requests[0].1["data"][0]["user_data"]["external_id"][0],
            "0198b202-8e18-7a21-8f31-e69167390312"
        );
        assert!(
            requests[1]
                .0
                .starts_with("/mp/collect?measurement_id=G-ABC123&api_secret=test-secret")
        );
        assert_eq!(requests[1].1["events"][0]["name"], "purchase");
        assert_eq!(
            requests[1].1["events"][0]["params"]["transaction_id"],
            "0198b202-8e18-7a21-8f31-e69167390311"
        );
        assert_eq!(requests[1].1["events"][0]["params"]["value"], 12.34);
        drop(requests);
        server.abort();
    }

    #[test]
    fn provider_errors_have_bounded_retry_classification() {
        assert!(http_error(StatusCode::TOO_MANY_REQUESTS).retryable);
        assert!(http_error(StatusCode::BAD_GATEWAY).retryable);
        assert!(!http_error(StatusCode::BAD_REQUEST).retryable);
        assert_eq!(major_number(1234, "JPY").unwrap(), Number::from(1234));
        assert_eq!(major_number(1234, "KWD").unwrap().to_string(), "1.234");
    }
}
