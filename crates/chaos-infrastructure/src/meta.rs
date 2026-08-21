//! Meta Conversions API delivery adapter.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chaos_application::ports::{
    AnalyticsDeliveryCommand, AnalyticsDeliveryError, AnalyticsDeliveryReceipt,
    AnalyticsEventDestination,
};
use reqwest::{Client, StatusCode, Url};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::secret::DynamicSecretResolver;

pub struct MetaConversionsDestination {
    client: Client,
    api_base_url: Url,
    secrets: Arc<DynamicSecretResolver>,
}

impl MetaConversionsDestination {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<DynamicSecretResolver>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            api_base_url.scheme() == "https" || api_base_url.host_str() == Some("127.0.0.1"),
            "Meta API URL must use HTTPS or loopback HTTP"
        );
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            api_base_url,
            secrets,
        })
    }
}

#[async_trait]
impl AnalyticsEventDestination for MetaConversionsDestination {
    fn provider(&self) -> &'static str {
        "meta"
    }

    async fn send(
        &self,
        command: &AnalyticsDeliveryCommand,
    ) -> Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError> {
        let token = self
            .secrets
            .resolve_analytics(&command.credential_secret_reference)
            .await?;
        let endpoint = self
            .api_base_url
            .join(&format!("{}/events", command.external_account_reference))
            .map_err(|_| invalid_command())?;
        let payload = MetaRequest {
            data: [MetaEvent {
                event_name: meta_event_name(&command.event_name),
                event_time: command.occurred_at.unix_timestamp(),
                event_id: command.event_id.to_string(),
                action_source: "website",
                event_source_url: command.source_url.as_deref(),
                user_data: MetaUserData {
                    external_id: vec![sha256_hex(command.shopper_id.as_bytes())],
                },
                custom_data: custom_data(command),
            }],
            test_event_code: command
                .configuration
                .get("test_event_code")
                .and_then(Value::as_str),
        };
        let response = self
            .client
            .post(endpoint)
            .query(&[("access_token", token.expose_secret())])
            .json(&payload)
            .send()
            .await
            .map_err(|error| AnalyticsDeliveryError {
                retryable: true,
                message: format!("Meta request failed: {error}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AnalyticsDeliveryError {
                retryable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
                message: format!("Meta returned HTTP {status}"),
            });
        }
        let receipt: MetaResponse = response.json().await.map_err(|_| AnalyticsDeliveryError {
            retryable: true,
            message: "Meta returned an invalid response".into(),
        })?;
        if receipt.events_received != 1 {
            return Err(AnalyticsDeliveryError {
                retryable: true,
                message: "Meta did not acknowledge the event".into(),
            });
        }
        Ok(AnalyticsDeliveryReceipt {
            provider_reference: receipt.fbtrace_id,
        })
    }
}

#[derive(Serialize)]
struct MetaRequest<'a> {
    data: [MetaEvent<'a>; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    test_event_code: Option<&'a str>,
}

#[derive(Serialize)]
struct MetaEvent<'a> {
    event_name: &'a str,
    event_time: i64,
    event_id: String,
    action_source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_source_url: Option<&'a str>,
    user_data: MetaUserData,
    custom_data: Value,
}

#[derive(Serialize)]
struct MetaUserData {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    external_id: Vec<String>,
}

#[derive(Deserialize)]
struct MetaResponse {
    events_received: u32,
    fbtrace_id: Option<String>,
}

fn meta_event_name(name: &str) -> &str {
    match name {
        "page_view" => "PageView",
        "view_content" => "ViewContent",
        "search" => "Search",
        "add_to_cart" => "AddToCart",
        "initiate_checkout" => "InitiateCheckout",
        "add_payment_info" => "AddPaymentInfo",
        "purchase" => "Purchase",
        "refund" => "Refund",
        "view_duration" => "ViewDuration",
        _ => name,
    }
}

fn custom_data(command: &AnalyticsDeliveryCommand) -> Value {
    let mut data = command.properties.clone();
    if let Some(object) = data.as_object_mut() {
        // Traffic provenance remains a Chaos Analytics fact. Click identifiers
        // must not be forwarded as arbitrary Meta custom_data fields.
        object.remove("traffic");
        if let Some(value) = command.value_minor {
            let exponent = command
                .currency
                .as_deref()
                .map(currency_exponent)
                .unwrap_or(2);
            object.insert("value".into(), json!(value as f64 / 10_f64.powi(exponent)));
        }
        if let Some(currency) = &command.currency {
            object.insert("currency".into(), json!(currency));
        }
    }
    data
}

fn currency_exponent(currency: &str) -> i32 {
    match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX" | "VND"
        | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "JOD" | "KWD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid_command() -> AnalyticsDeliveryError {
    AnalyticsDeliveryError {
        retryable: false,
        message: "Meta delivery command is invalid".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn command(value_minor: i64, currency: &str) -> AnalyticsDeliveryCommand {
        AnalyticsDeliveryCommand {
            delivery_id: Uuid::now_v7(),
            provider: "meta".into(),
            event_id: Uuid::now_v7(),
            external_account_reference: "12345".into(),
            credential_secret_reference: "env://CHAOS_ANALYTICS_SECRET_TEST".into(),
            configuration: json!({}),
            event_name: "purchase".into(),
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            shopper_id: Uuid::now_v7(),
            source_url: None,
            value_minor: Some(value_minor),
            currency: Some(currency.into()),
            properties: json!({}),
        }
    }

    #[test]
    fn converts_minor_units_for_meta_without_changing_currency() {
        let usd = custom_data(&command(1_299, "USD"));
        assert_eq!(usd["value"], json!(12.99));
        assert_eq!(usd["currency"], json!("USD"));

        let jpy = custom_data(&command(1_299, "JPY"));
        assert_eq!(jpy["value"], json!(1_299.0));
        assert_eq!(jpy["currency"], json!("JPY"));
    }

    #[test]
    fn does_not_forward_traffic_provenance_as_meta_custom_data() {
        let mut input = command(1_299, "USD");
        input.properties = json!({"traffic":{"session":{"fbclid":"private"}},"path":"/"});
        let data = custom_data(&input);
        assert!(data.get("traffic").is_none());
        assert_eq!(data["path"], json!("/"));
    }
}
