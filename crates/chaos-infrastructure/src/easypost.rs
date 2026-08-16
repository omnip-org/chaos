use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chaos_application::{
    ApplicationError,
    ports::{
        CancelShippingLabelCommand, ProviderTrackingStatus, PurchaseShippingLabelCommand,
        PurchasedShippingLabel, ReconcileShippingLabelCommand, ReconciledShippingLabel,
        RefreshTrackingCommand, ShippingAddress, ShippingCancellationStatus, ShippingProvider,
        ShippingQuoteCommand, ShippingRateQuote, ShippingSecretResolver, ShippingTrackingSnapshot,
    },
};
use chaos_domain::{CurrencyCode, fulfillment::ShippingSecretReference};
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use time::OffsetDateTime;

pub struct EasyPostShippingProvider {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn ShippingSecretResolver>,
}

impl EasyPostShippingProvider {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn ShippingSecretResolver>,
    ) -> anyhow::Result<Self> {
        if api_base_url.scheme() != "https" && !api_base_url.host_str().is_some_and(is_loopback) {
            anyhow::bail!("EasyPost API base URL must use HTTPS outside loopback tests");
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            api_base_url,
            secrets,
        })
    }

    async fn credentials(
        &self,
        reference: &ShippingSecretReference,
    ) -> Result<SecretString, ApplicationError> {
        let secret = self.secrets.resolve(reference).await?;
        if !secret.expose_secret().starts_with("EZ") || secret.expose_secret().len() > 255 {
            return Err(secret_unavailable());
        }
        Ok(secret)
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApplicationError> {
        self.api_base_url
            .join(path)
            .map_err(|error| ApplicationError::Unexpected(error.into()))
    }

    async fn post<T: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        path: &str,
        secret: &SecretString,
        payload: Option<&T>,
    ) -> Result<R, ApplicationError> {
        let mut request = self
            .client
            .post(self.endpoint(path)?)
            .headers(easypost_headers(secret)?);
        if let Some(payload) = payload {
            request = request.json(payload);
        }
        parse_response(request.send().await.map_err(network_error)?).await
    }

    async fn get<R: DeserializeOwned>(
        &self,
        path: &str,
        secret: &SecretString,
    ) -> Result<R, ApplicationError> {
        parse_response(
            self.client
                .get(self.endpoint(path)?)
                .headers(easypost_headers(secret)?)
                .send()
                .await
                .map_err(network_error)?,
        )
        .await
    }
}

#[async_trait]
impl ShippingProvider for EasyPostShippingProvider {
    fn name(&self) -> &'static str {
        "easypost"
    }

    async fn quote_rates(
        &self,
        command: ShippingQuoteCommand,
    ) -> Result<Vec<ShippingRateQuote>, ApplicationError> {
        validate_idempotency_key(&command.idempotency_key)?;
        validate_parcel(command.parcel)?;
        let secret = self
            .credentials(&command.credential_secret_reference)
            .await?;
        let request = CreateShipmentRequest {
            shipment: CreateShipment {
                reference: &command.idempotency_key,
                from_address: EasyPostAddress::from(&command.from),
                to_address: EasyPostAddress::from(&command.to),
                parcel: EasyPostParcel {
                    length: inches(command.parcel.length_millimetres),
                    width: inches(command.parcel.width_millimetres),
                    height: inches(command.parcel.height_millimetres),
                    weight: ounces(command.parcel.weight_grams),
                },
            },
        };
        let shipment: EasyPostShipment = self.post("v2/shipments", &secret, Some(&request)).await?;
        validate_reference(&shipment.id, "shp_")?;
        shipment
            .rates
            .into_iter()
            .map(|rate| {
                validate_reference(&rate.id, "rate_")?;
                let currency = CurrencyCode::parse(&rate.currency)?;
                Ok(ShippingRateQuote {
                    provider_shipment_reference: shipment.id.clone(),
                    provider_rate_reference: rate.id,
                    carrier: bounded_text(rate.carrier, "carrier", 100)?,
                    service: bounded_text(rate.service, "service", 120)?,
                    amount_minor: parse_major_amount(&rate.rate, currency)?,
                    currency,
                    estimated_delivery_days: rate
                        .delivery_days
                        .or(rate.est_delivery_days)
                        .map(|days| {
                            u16::try_from(days)
                                .map_err(|error| ApplicationError::Unexpected(error.into()))
                        })
                        .transpose()?,
                    guaranteed: rate.delivery_date_guaranteed.unwrap_or(false),
                })
            })
            .collect()
    }

    async fn purchase_label(
        &self,
        command: PurchaseShippingLabelCommand,
    ) -> Result<PurchasedShippingLabel, ApplicationError> {
        validate_idempotency_key(&command.idempotency_key)?;
        validate_reference(&command.provider_shipment_reference, "shp_")?;
        validate_reference(&command.provider_rate_reference, "rate_")?;
        let secret = self
            .credentials(&command.credential_secret_reference)
            .await?;
        let request = BuyShipmentRequest {
            rate: BuyRate {
                id: &command.provider_rate_reference,
            },
        };
        let shipment: EasyPostShipment = self
            .post(
                &format!("v2/shipments/{}/buy", command.provider_shipment_reference),
                &secret,
                Some(&request),
            )
            .await?;
        purchased_label(shipment)?.ok_or_else(invalid_response)
    }

    async fn reconcile_label(
        &self,
        command: ReconcileShippingLabelCommand,
    ) -> Result<ReconciledShippingLabel, ApplicationError> {
        validate_reference(&command.provider_shipment_reference, "shp_")?;
        let secret = self
            .credentials(&command.credential_secret_reference)
            .await?;
        let shipment: EasyPostShipment = self
            .get(
                &format!("v2/shipments/{}", command.provider_shipment_reference),
                &secret,
            )
            .await?;
        let cancellation_status = shipment
            .refund_status
            .as_deref()
            .map(cancellation_status)
            .transpose()?;
        Ok(ReconciledShippingLabel {
            label: purchased_label(shipment)?,
            cancellation_status,
        })
    }

    async fn cancel_label(
        &self,
        command: CancelShippingLabelCommand,
    ) -> Result<ShippingCancellationStatus, ApplicationError> {
        validate_reference(&command.provider_shipment_reference, "shp_")?;
        let secret = self
            .credentials(&command.credential_secret_reference)
            .await?;
        let shipment: EasyPostShipment = self
            .post::<serde_json::Value, _>(
                &format!(
                    "v2/shipments/{}/refund",
                    command.provider_shipment_reference
                ),
                &secret,
                None,
            )
            .await?;
        cancellation_status(
            shipment
                .refund_status
                .as_deref()
                .ok_or_else(invalid_response)?,
        )
    }

    async fn refresh_tracking(
        &self,
        command: RefreshTrackingCommand,
        observed_at: OffsetDateTime,
    ) -> Result<ShippingTrackingSnapshot, ApplicationError> {
        validate_reference(&command.provider_tracker_reference, "trk_")?;
        let secret = self
            .credentials(&command.credential_secret_reference)
            .await?;
        let tracker: EasyPostTracker = self
            .get(
                &format!("v2/trackers/{}", command.provider_tracker_reference),
                &secret,
            )
            .await?;
        validate_reference(&tracker.id, "trk_")?;
        Ok(ShippingTrackingSnapshot {
            status: match tracker.status.as_str() {
                "pre_transit" => ProviderTrackingStatus::PreTransit,
                "in_transit" => ProviderTrackingStatus::InTransit,
                "out_for_delivery" => ProviderTrackingStatus::OutForDelivery,
                "delivered" => ProviderTrackingStatus::Delivered,
                "failure" | "return_to_sender" => ProviderTrackingStatus::Failure,
                "unknown" | "available_for_pickup" | "cancelled" | "error" => {
                    ProviderTrackingStatus::Unknown
                }
                _ => return Err(invalid_response()),
            },
            status_detail: tracker
                .status_detail
                .map(|value| bounded_text(value, "status_detail", 255))
                .transpose()?,
            estimated_delivery_at: tracker.est_delivery_date,
            observed_at,
        })
    }
}

#[derive(Serialize)]
struct CreateShipmentRequest<'a> {
    shipment: CreateShipment<'a>,
}

#[derive(Serialize)]
struct CreateShipment<'a> {
    reference: &'a str,
    from_address: EasyPostAddress<'a>,
    to_address: EasyPostAddress<'a>,
    parcel: EasyPostParcel,
}

#[derive(Serialize)]
struct EasyPostAddress<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    company: Option<&'a str>,
    street1: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    street2: Option<&'a str>,
    city: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
    zip: &'a str,
    country: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
}

impl<'a> From<&'a ShippingAddress> for EasyPostAddress<'a> {
    fn from(value: &'a ShippingAddress) -> Self {
        Self {
            name: &value.name,
            company: value.company.as_deref(),
            street1: &value.address_line_1,
            street2: value.address_line_2.as_deref(),
            city: &value.city,
            state: value.region.as_deref(),
            zip: &value.postal_code,
            country: &value.country_code,
            phone: value.phone.as_deref(),
            email: value.email.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct EasyPostParcel {
    length: String,
    width: String,
    height: String,
    weight: String,
}

#[derive(Serialize)]
struct BuyShipmentRequest<'a> {
    rate: BuyRate<'a>,
}

#[derive(Serialize)]
struct BuyRate<'a> {
    id: &'a str,
}

#[derive(Deserialize)]
struct EasyPostShipment {
    id: String,
    #[serde(default)]
    rates: Vec<EasyPostRate>,
    selected_rate: Option<EasyPostRate>,
    tracking_code: Option<String>,
    postage_label: Option<EasyPostLabel>,
    tracker: Option<EasyPostTrackerReference>,
    refund_status: Option<String>,
}

#[derive(Deserialize)]
struct EasyPostRate {
    id: String,
    carrier: String,
    service: String,
    rate: String,
    currency: String,
    delivery_days: Option<i64>,
    est_delivery_days: Option<i64>,
    delivery_date_guaranteed: Option<bool>,
}

#[derive(Deserialize)]
struct EasyPostLabel {
    label_url: String,
    label_file_type: String,
}

#[derive(Deserialize)]
struct EasyPostTrackerReference {
    id: String,
}

#[derive(Deserialize)]
struct EasyPostTracker {
    id: String,
    status: String,
    status_detail: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    est_delivery_date: Option<OffsetDateTime>,
}

fn easypost_headers(secret: &SecretString) -> Result<HeaderMap, ApplicationError> {
    let encoded = STANDARD.encode(format!("{}:", secret.expose_secret()));
    let mut authorization = HeaderValue::from_str(&format!("Basic {encoded}"))
        .map_err(|error| ApplicationError::Unexpected(error.into()))?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(headers)
}

async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T, ApplicationError> {
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(ApplicationError::Unavailable {
            service: "shipping_provider",
            source: anyhow::anyhow!("EasyPost returned HTTP {status}"),
        });
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(secret_unavailable());
    }
    if !status.is_success() {
        return Err(ApplicationError::Conflict {
            code: "shipping_provider_rejected",
            message: "The shipping provider rejected the request",
        });
    }
    response
        .json()
        .await
        .map_err(|error| ApplicationError::Unavailable {
            service: "shipping_provider",
            source: error.into(),
        })
}

fn parse_major_amount(value: &str, currency: CurrencyCode) -> Result<i64, ApplicationError> {
    let exponent = currency_exponent(currency.as_str());
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_response());
    }
    let exponent = usize::from(exponent);
    if fraction.len() > exponent && !fraction[exponent..].bytes().all(|byte| byte == b'0') {
        return Err(invalid_response());
    }
    let whole = whole.parse::<i64>().map_err(|_| invalid_response())?;
    let scale = 10_i64
        .checked_pow(exponent as u32)
        .ok_or_else(invalid_response)?;
    let mut normalized = fraction[..fraction.len().min(exponent)].to_owned();
    normalized.extend(std::iter::repeat_n(
        '0',
        exponent.saturating_sub(normalized.len()),
    ));
    let fraction = if normalized.is_empty() {
        0
    } else {
        normalized.parse::<i64>().map_err(|_| invalid_response())?
    };
    whole
        .checked_mul(scale)
        .and_then(|amount| amount.checked_add(fraction))
        .ok_or_else(invalid_response)
}

fn currency_exponent(currency: &str) -> u8 {
    match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" | "UYW" => 4,
        _ => 2,
    }
}

fn validate_parcel(
    parcel: chaos_application::ports::ShippingParcel,
) -> Result<(), ApplicationError> {
    if parcel.length_millimetres == 0
        || parcel.width_millimetres == 0
        || parcel.height_millimetres == 0
        || parcel.weight_grams == 0
        || parcel.length_millimetres > 10_000
        || parcel.width_millimetres > 10_000
        || parcel.height_millimetres > 10_000
        || parcel.weight_grams > 1_000_000
    {
        return Err(ApplicationError::Conflict {
            code: "invalid_shipping_parcel",
            message: "The shipping parcel dimensions or weight are invalid",
        });
    }
    Ok(())
}

fn inches(millimetres: u32) -> String {
    format!("{:.4}", f64::from(millimetres) / 25.4)
}

fn ounces(grams: u32) -> String {
    format!("{:.4}", f64::from(grams) / 28.349_523_125)
}

fn validate_idempotency_key(value: &str) -> Result<(), ApplicationError> {
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_whitespace) {
        return Err(ApplicationError::Conflict {
            code: "invalid_shipping_idempotency_key",
            message: "The shipping idempotency key is invalid",
        });
    }
    Ok(())
}

fn validate_reference(value: &str, prefix: &str) -> Result<(), ApplicationError> {
    if !value.starts_with(prefix)
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid_response());
    }
    Ok(())
}

fn validate_https_url(value: String) -> Result<String, ApplicationError> {
    let url = Url::parse(&value).map_err(|_| invalid_response())?;
    if url.scheme() != "https" || url.host_str().is_none() || url.username() != "" {
        return Err(invalid_response());
    }
    Ok(value)
}

fn purchased_label(
    shipment: EasyPostShipment,
) -> Result<Option<PurchasedShippingLabel>, ApplicationError> {
    validate_reference(&shipment.id, "shp_")?;
    let Some(label) = shipment.postage_label else {
        return Ok(None);
    };
    let selected_rate = shipment.selected_rate.ok_or_else(invalid_response)?;
    let tracking_number = bounded_text(
        shipment.tracking_code.ok_or_else(invalid_response)?,
        "tracking_number",
        255,
    )?;
    Ok(Some(PurchasedShippingLabel {
        provider_shipment_reference: shipment.id,
        carrier: bounded_text(selected_rate.carrier, "carrier", 100)?,
        tracking_number,
        provider_tracker_reference: shipment
            .tracker
            .map(|tracker| validate_reference(&tracker.id, "trk_").map(|()| tracker.id))
            .transpose()?,
        label_url: validate_https_url(label.label_url)?,
        label_media_type: bounded_text(label.label_file_type, "label_media_type", 100)?,
    }))
}

fn cancellation_status(value: &str) -> Result<ShippingCancellationStatus, ApplicationError> {
    match value {
        "submitted" => Ok(ShippingCancellationStatus::Submitted),
        "refunded" => Ok(ShippingCancellationStatus::Cancelled),
        "rejected" => Ok(ShippingCancellationStatus::Rejected),
        "not_applicable" => Ok(ShippingCancellationStatus::NotAvailable),
        _ => Err(invalid_response()),
    }
}

fn bounded_text(
    value: String,
    _field: &'static str,
    maximum: usize,
) -> Result<String, ApplicationError> {
    if value.trim().is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(invalid_response());
    }
    Ok(value)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn network_error(error: reqwest::Error) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "shipping_provider",
        source: error.into(),
    }
}

fn secret_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "shipping_provider_credentials_unavailable",
        message: "The shipping provider credentials are unavailable",
    }
}

fn invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "shipping_provider",
        source: anyhow::anyhow!("EasyPost returned an invalid response"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        body::Bytes,
        extract::{OriginalUri, State},
        http::{HeaderMap, Method, StatusCode},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use chaos_application::ports::{ShippingParcel, ShippingSecretResolver};

    use super::*;

    struct StaticSecrets;

    #[async_trait]
    impl ShippingSecretResolver for StaticSecrets {
        async fn resolve(
            &self,
            _reference: &ShippingSecretReference,
        ) -> Result<SecretString, ApplicationError> {
            Ok(SecretString::from("EZAK_test_key"))
        }
    }

    type RecordedRequest = (Method, String, String, Value);

    #[derive(Clone, Default)]
    struct Requests(Arc<Mutex<Vec<RecordedRequest>>>);

    async fn easypost_mock(
        State(requests): State<Requests>,
        method: Method,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<Value>) {
        let payload = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).expect("request JSON")
        };
        requests.0.lock().expect("requests").push((
            method.clone(),
            uri.path().to_owned(),
            headers[AUTHORIZATION]
                .to_str()
                .expect("authorization")
                .to_owned(),
            payload,
        ));
        let response = match (method, uri.path()) {
            (Method::POST, "/v2/shipments") => json!({
                "id": "shp_quote123",
                "rates": [{
                    "id": "rate_ground123",
                    "carrier": "USPS",
                    "service": "GroundAdvantage",
                    "rate": "12.34",
                    "currency": "USD",
                    "delivery_days": 4,
                    "est_delivery_days": 5,
                    "delivery_date_guaranteed": false
                }],
                "selected_rate": null,
                "tracking_code": null,
                "postage_label": null,
                "tracker": null,
                "refund_status": null
            }),
            (Method::POST, "/v2/shipments/shp_quote123/buy")
            | (Method::GET, "/v2/shipments/shp_quote123") => json!({
                "id": "shp_quote123",
                "rates": [],
                "selected_rate": {
                    "id": "rate_ground123",
                    "carrier": "USPS",
                    "service": "GroundAdvantage",
                    "rate": "12.34",
                    "currency": "USD",
                    "delivery_days": 4,
                    "est_delivery_days": 5,
                    "delivery_date_guaranteed": false
                },
                "tracking_code": "9400111899223856928499",
                "postage_label": {
                    "label_url": "https://labels.example.com/label.png",
                    "label_file_type": "image/png"
                },
                "tracker": {"id": "trk_label123"},
                "refund_status": null
            }),
            (Method::POST, "/v2/shipments/shp_quote123/refund") => json!({
                "id": "shp_quote123",
                "rates": [],
                "selected_rate": null,
                "tracking_code": "9400111899223856928499",
                "postage_label": null,
                "tracker": null,
                "refund_status": "submitted"
            }),
            (Method::GET, "/v2/trackers/trk_label123") => json!({
                "id": "trk_label123",
                "status": "in_transit",
                "status_detail": "arrived_at_facility",
                "est_delivery_date": "2026-08-20T12:00:00Z"
            }),
            _ => return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))),
        };
        (StatusCode::OK, Json(response))
    }

    async fn provider() -> (
        EasyPostShippingProvider,
        Requests,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Requests::default();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let app = Router::new()
            .route("/v2/shipments", post(easypost_mock))
            .route("/v2/shipments/{id}", get(easypost_mock))
            .route("/v2/shipments/{id}/buy", post(easypost_mock))
            .route("/v2/shipments/{id}/refund", post(easypost_mock))
            .route("/v2/trackers/{id}", get(easypost_mock))
            .with_state(requests.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        let provider = EasyPostShippingProvider::new(
            Url::parse(&format!("http://127.0.0.1:{}/", address.port())).expect("URL"),
            Duration::from_secs(1),
            Arc::new(StaticSecrets),
        )
        .expect("provider");
        (provider, requests, server)
    }

    fn address(name: &str, country_code: &str) -> ShippingAddress {
        ShippingAddress {
            name: name.into(),
            company: None,
            address_line_1: "10 Main Street".into(),
            address_line_2: None,
            city: "Singapore".into(),
            region: None,
            postal_code: "018956".into(),
            country_code: country_code.into(),
            phone: Some("+6591234567".into()),
            email: Some("shipping@example.com".into()),
        }
    }

    fn secret_reference() -> ShippingSecretReference {
        ShippingSecretReference::new("env://CHAOS_SHIPPING_SECRET_EASYPOST").expect("reference")
    }

    #[tokio::test]
    async fn easypost_quotes_buys_refunds_and_refreshes_tracking() {
        let (provider, requests, server) = provider().await;
        let rates = provider
            .quote_rates(ShippingQuoteCommand {
                from: address("Warehouse", "SG"),
                to: address("Shopper", "US"),
                parcel: ShippingParcel {
                    length_millimetres: 254,
                    width_millimetres: 127,
                    height_millimetres: 51,
                    weight_grams: 454,
                },
                idempotency_key: "shipping-quote-019123".into(),
                credential_secret_reference: secret_reference(),
            })
            .await
            .expect("rates");
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].amount_minor, 1234);
        assert_eq!(rates[0].currency.as_str(), "USD");
        assert_eq!(rates[0].estimated_delivery_days, Some(4));

        let label = provider
            .purchase_label(PurchaseShippingLabelCommand {
                provider_shipment_reference: rates[0].provider_shipment_reference.clone(),
                provider_rate_reference: rates[0].provider_rate_reference.clone(),
                idempotency_key: "shipping-label-019123".into(),
                credential_secret_reference: secret_reference(),
            })
            .await
            .expect("label");
        assert_eq!(label.carrier, "USPS");
        assert_eq!(
            label.provider_tracker_reference.as_deref(),
            Some("trk_label123")
        );
        assert_eq!(label.label_media_type, "image/png");

        let reconciled = provider
            .reconcile_label(ReconcileShippingLabelCommand {
                provider_shipment_reference: "shp_quote123".into(),
                credential_secret_reference: secret_reference(),
            })
            .await
            .expect("reconcile label");
        assert_eq!(
            reconciled
                .label
                .as_ref()
                .map(|label| label.tracking_number.as_str()),
            Some("9400111899223856928499")
        );

        let cancellation = provider
            .cancel_label(CancelShippingLabelCommand {
                provider_shipment_reference: label.provider_shipment_reference,
                credential_secret_reference: secret_reference(),
            })
            .await
            .expect("refund label");
        assert_eq!(cancellation, ShippingCancellationStatus::Submitted);

        let observed_at = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time");
        let tracking = provider
            .refresh_tracking(
                RefreshTrackingCommand {
                    provider_tracker_reference: "trk_label123".into(),
                    credential_secret_reference: secret_reference(),
                },
                observed_at,
            )
            .await
            .expect("tracking");
        assert_eq!(tracking.status, ProviderTrackingStatus::InTransit);
        assert_eq!(tracking.observed_at, observed_at);

        let requests = requests.0.lock().expect("requests");
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| {
            request.2 == format!("Basic {}", STANDARD.encode("EZAK_test_key:"))
        }));
        assert_eq!(
            requests[0].3["shipment"]["reference"],
            "shipping-quote-019123"
        );
        assert_eq!(requests[0].3["shipment"]["parcel"]["length"], "10.0000");
        assert_eq!(requests[1].3["rate"]["id"], "rate_ground123");
        server.abort();
    }

    #[test]
    fn currency_conversion_respects_iso_minor_units_without_rounding_money() {
        assert_eq!(
            parse_major_amount("123", CurrencyCode::parse("JPY").unwrap()).unwrap(),
            123
        );
        assert_eq!(
            parse_major_amount("1.234", CurrencyCode::parse("KWD").unwrap()).unwrap(),
            1234
        );
        assert!(parse_major_amount("1.001", CurrencyCode::parse("USD").unwrap()).is_err());
    }
}
