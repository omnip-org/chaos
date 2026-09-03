use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{
    ApplicationError,
    contracts::{
        IntegrationSecretResolver, PaymentClientAction, PaymentCommand, PaymentCommandKind,
        PaymentCommandResult, PaymentProvider, PaymentRefundObservation, PaymentRefundStatus,
        PaymentShippingAddress, PaymentWebhookVerifier, StripeWebhookConfigurationRepository,
        StripeWebhookEvent,
    },
};
use async_trait::async_trait;
use chaos_domain::{CurrencyCode, stripe::StripeAccountId};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::Sha256;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

const STRIPE_API_VERSION: &str = "2026-07-29.dahlia";
const WEBHOOK_TOLERANCE_SECONDS: i64 = 300;

/// Stripe API transport. Payment and refund commands intentionally share one
/// concrete gateway because this deployment is Stripe-only.
struct StripeHttp {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn IntegrationSecretResolver>,
}

impl StripeHttp {
    fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn IntegrationSecretResolver>,
    ) -> Result<Self, anyhow::Error> {
        if api_base_url.scheme() != "https" && !api_base_url.host_str().is_some_and(is_loopback) {
            anyhow::bail!("Stripe API base URL must use HTTPS outside loopback tests");
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            api_base_url,
            secrets,
        })
    }

    async fn credentials(&self, reference: &str) -> Result<StripeCredentials, ApplicationError> {
        let secret = self.secrets.resolve(reference).await?;
        let credentials: StripeCredentials =
            serde_json::from_str(secret.expose_secret()).map_err(|_| secret_unavailable())?;
        if !credentials.secret_key.expose_secret().starts_with("sk_")
            || !credentials
                .publishable_key
                .expose_secret()
                .starts_with("pk_")
        {
            return Err(secret_unavailable());
        }
        Ok(credentials)
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApplicationError> {
        self.api_base_url
            .join(path)
            .map_err(|error| ApplicationError::Unexpected(error.into()))
    }

    async fn send_form(
        &self,
        path: &str,
        credentials: &StripeCredentials,
        idempotency_key: &str,
        form: &[(String, String)],
    ) -> Result<StripeObject, ApplicationError> {
        let response = self
            .client
            .post(self.endpoint(path)?)
            .headers(stripe_headers(
                credentials.secret_key.expose_secret(),
                Some(idempotency_key),
            )?)
            .form(form)
            .send()
            .await
            .map_err(provider_network_error)?;
        parse_stripe_response(response).await
    }

    /// Retrieves an object by id, validating the id carries `expected_prefix`
    /// (e.g. `"pi_"` for PaymentIntents, `"cs_"` for Checkout Sessions)
    /// before making the request.
    async fn retrieve_object(
        &self,
        path_prefix: &str,
        credentials: &StripeCredentials,
        stripe_reference: &str,
        expected_prefix: &str,
    ) -> Result<StripeObject, ApplicationError> {
        if !valid_stripe_identifier(stripe_reference, expected_prefix) {
            return Err(stripe_invalid_response());
        }
        let response = self
            .client
            .get(self.endpoint(&format!("{path_prefix}{stripe_reference}"))?)
            .headers(stripe_headers(
                credentials.secret_key.expose_secret(),
                None,
            )?)
            .send()
            .await
            .map_err(provider_network_error)?;
        parse_stripe_response(response).await
    }

    async fn list_refunds(
        &self,
        credentials: &StripeCredentials,
        payment_intent: &str,
    ) -> Result<Vec<StripeRefundObject>, ApplicationError> {
        if !valid_stripe_identifier(payment_intent, "pi_") {
            return Err(stripe_invalid_response());
        }
        let mut refunds = Vec::new();
        let mut starting_after: Option<String> = None;
        loop {
            let mut query = vec![
                ("payment_intent".to_owned(), payment_intent.to_owned()),
                ("limit".to_owned(), "100".to_owned()),
            ];
            if let Some(cursor) = starting_after.as_ref() {
                query.push(("starting_after".to_owned(), cursor.clone()));
            }
            let response = self
                .client
                .get(self.endpoint("v1/refunds")?)
                .headers(stripe_headers(
                    credentials.secret_key.expose_secret(),
                    None,
                )?)
                .query(&query)
                .send()
                .await
                .map_err(provider_network_error)?;
            let page: StripeRefundList = parse_stripe_response(response).await?;
            let has_more = page.has_more;
            let next_cursor = page.data.last().map(|refund| refund.id.clone());
            refunds.extend(page.data);
            if !has_more {
                break;
            }
            starting_after = Some(next_cursor.ok_or_else(stripe_invalid_response)?);
        }
        Ok(refunds)
    }
}

pub struct StripeGateway {
    http: StripeHttp,
}

impl StripeGateway {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn IntegrationSecretResolver>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            http: StripeHttp::new(api_base_url, timeout, secrets)?,
        })
    }
}

#[async_trait]
impl PaymentProvider for StripeGateway {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn execute(
        &self,
        command: PaymentCommand,
    ) -> Result<PaymentCommandResult, ApplicationError> {
        let credentials = self
            .http
            .credentials(&command.credential_secret_reference)
            .await?;
        if command.kind == PaymentCommandKind::CreateRefund {
            let payment_reference = command.provider_payment_reference.as_deref().ok_or(
                ApplicationError::Conflict {
                    code: "stripe_payment_intent_missing",
                    message: "the captured Stripe payment has no PaymentIntent",
                },
            )?;
            let payment_intent = if valid_stripe_identifier(payment_reference, "pi_") {
                payment_reference.to_owned()
            } else {
                let session = self
                    .http
                    .retrieve_object(
                        "v1/checkout/sessions/",
                        &credentials,
                        payment_reference,
                        "cs_",
                    )
                    .await?;
                session
                    .payment_intent
                    .filter(|value| valid_stripe_identifier(value, "pi_"))
                    .ok_or(ApplicationError::Conflict {
                        code: "stripe_payment_intent_missing",
                        message: "the Stripe Checkout Session has no PaymentIntent",
                    })?
            };
            let object = self
                .http
                .send_form(
                    "v1/refunds",
                    &credentials,
                    &command.idempotency_key,
                    &[
                        ("payment_intent".into(), payment_intent),
                        ("amount".into(), command.amount_minor.to_string()),
                        (
                            "metadata[chaos_order_id]".into(),
                            command.aggregate_id.to_string(),
                        ),
                        (
                            "metadata[chaos_refund_id]".into(),
                            command
                                .refund_id
                                .ok_or_else(stripe_invalid_response)?
                                .to_string(),
                        ),
                        (
                            "metadata[chaos_store_id]".into(),
                            command.order_context.store_id.to_string(),
                        ),
                        (
                            "metadata[chaos_shopper_id]".into(),
                            command.order_context.shopper_id.to_string(),
                        ),
                        (
                            "metadata[chaos_channel_id]".into(),
                            command.order_context.channel_id.to_string(),
                        ),
                        (
                            "metadata[chaos_order_number]".into(),
                            command.order_context.order_number.clone(),
                        ),
                    ],
                )
                .await?;
            if !valid_stripe_identifier(&object.id, "re_") {
                return Err(stripe_invalid_response());
            }
            return Ok(PaymentCommandResult {
                provider_object_id: object.id,
                client_action: None,
            });
        }
        if command.kind != PaymentCommandKind::CreateCheckoutSession {
            return Err(stripe_invalid_response());
        }
        let return_url = command
            .return_url
            .as_deref()
            .ok_or_else(stripe_invalid_response)?;
        let checkout_details = command
            .checkout_details
            .as_ref()
            .ok_or_else(stripe_invalid_response)?;
        let mut form = vec![
            ("mode".into(), "payment".into()),
            ("ui_mode".into(), "embedded_page".into()),
            ("return_url".into(), return_url.into()),
            ("phone_number_collection[enabled]".into(), "true".into()),
            ("billing_address_collection".into(), "required".into()),
            ("allow_promotion_codes".into(), "true".into()),
            (
                "automatic_tax[enabled]".into(),
                checkout_details.automatic_tax.to_string(),
            ),
            (
                "metadata[chaos_order_id]".into(),
                command.aggregate_id.to_string(),
            ),
            (
                "metadata[chaos_store_id]".into(),
                command.order_context.store_id.to_string(),
            ),
            (
                "metadata[chaos_shopper_id]".into(),
                command.order_context.shopper_id.to_string(),
            ),
            (
                "metadata[chaos_channel_id]".into(),
                command.order_context.channel_id.to_string(),
            ),
            (
                "metadata[chaos_order_number]".into(),
                command.order_context.order_number.clone(),
            ),
        ];
        if let Some(customer_email) = checkout_details.customer_email.as_deref() {
            form.push(("customer_email".into(), customer_email.into()));
        }
        for (index, line) in checkout_details.line_items.iter().enumerate() {
            form.push((
                format!("line_items[{index}][quantity]"),
                line.quantity.to_string(),
            ));
            form.push((
                format!("line_items[{index}][price_data][currency]"),
                command.currency.as_str().to_ascii_lowercase(),
            ));
            form.push((
                format!("line_items[{index}][price_data][unit_amount]"),
                line.unit_amount_minor.to_string(),
            ));
            form.push((
                format!("line_items[{index}][price_data][tax_behavior]"),
                "exclusive".into(),
            ));
            form.push((
                format!("line_items[{index}][price_data][product_data][name]"),
                line.name.clone(),
            ));
            if let Some(sku) = line.sku.as_deref() {
                form.push((
                    format!("line_items[{index}][price_data][product_data][metadata][sku]"),
                    sku.into(),
                ));
            }
            if let Some(image_url) = line.image_url.as_deref() {
                form.push((
                    format!("line_items[{index}][price_data][product_data][images][0]"),
                    image_url.into(),
                ));
            }
        }
        for country in &checkout_details.shipping_countries {
            form.push((
                "shipping_address_collection[allowed_countries][]".into(),
                country.clone(),
            ));
        }
        for (index, option) in checkout_details.shipping_options.iter().enumerate() {
            let prefix = format!("shipping_options[{index}][shipping_rate_data]");
            form.push((format!("{prefix}[display_name]"), option.name.clone()));
            form.push((format!("{prefix}[type]"), "fixed_amount".into()));
            form.push((
                format!("{prefix}[fixed_amount][amount]"),
                option.amount_minor.to_string(),
            ));
            form.push((
                format!("{prefix}[fixed_amount][currency]"),
                option.currency.as_str().to_ascii_lowercase(),
            ));
            form.push((
                format!("{prefix}[delivery_estimate][minimum][unit]"),
                "business_day".into(),
            ));
            form.push((
                format!("{prefix}[delivery_estimate][minimum][value]"),
                option.estimated_min_days.to_string(),
            ));
            form.push((
                format!("{prefix}[delivery_estimate][maximum][unit]"),
                "business_day".into(),
            ));
            form.push((
                format!("{prefix}[delivery_estimate][maximum][value]"),
                option.estimated_max_days.to_string(),
            ));
            form.push((
                format!("{prefix}[metadata][chaos_shipping_rate_id]"),
                option.service_id.to_string(),
            ));
        }
        if let Some(customer_email) = checkout_details.customer_email.as_deref() {
            form.push((
                "payment_intent_data[receipt_email]".into(),
                customer_email.into(),
            ));
        }
        if let Some(shipping) = checkout_details.shipping_address.as_ref() {
            append_shipping_address(
                &mut form,
                shipping,
                checkout_details.customer_phone.as_deref(),
            );
        }
        let object = self
            .http
            .send_form(
                "v1/checkout/sessions",
                &credentials,
                &command.idempotency_key,
                &form,
            )
            .await?;
        if !valid_stripe_identifier(&object.id, "cs_") {
            return Err(stripe_invalid_response());
        }
        let client_secret = object.client_secret.ok_or_else(stripe_invalid_response)?;
        Ok(PaymentCommandResult {
            provider_object_id: object.id,
            client_action: Some(PaymentClientAction {
                kind: "mount_embedded_checkout",
                public_key: credentials.publishable_key,
                client_token: SecretString::from(client_secret),
            }),
        })
    }

    async fn list_refunds(
        &self,
        credential_secret_reference: &str,
        payment_provider_reference: &str,
    ) -> Result<Vec<PaymentRefundObservation>, ApplicationError> {
        let credentials = self.http.credentials(credential_secret_reference).await?;
        let payment_intent = if valid_stripe_identifier(payment_provider_reference, "pi_") {
            payment_provider_reference.to_owned()
        } else {
            let session = self
                .http
                .retrieve_object(
                    "v1/checkout/sessions/",
                    &credentials,
                    payment_provider_reference,
                    "cs_",
                )
                .await?;
            session
                .payment_intent
                .filter(|value| valid_stripe_identifier(value, "pi_"))
                .ok_or(ApplicationError::Conflict {
                    code: "stripe_payment_intent_missing",
                    message: "the Stripe Checkout Session has no PaymentIntent",
                })?
        };
        self.http
            .list_refunds(&credentials, &payment_intent)
            .await?
            .into_iter()
            .map(payment_refund_observation)
            .collect()
    }
}

fn append_shipping_address(
    form: &mut Vec<(String, String)>,
    shipping: &PaymentShippingAddress,
    phone: Option<&str>,
) {
    form.push((
        "payment_intent_data[shipping][name]".into(),
        shipping.name.clone(),
    ));
    form.push((
        "payment_intent_data[shipping][address][line1]".into(),
        shipping.line1.clone(),
    ));
    form.push((
        "payment_intent_data[shipping][address][city]".into(),
        shipping.city.clone(),
    ));
    form.push((
        "payment_intent_data[shipping][address][country]".into(),
        shipping.country_code.clone(),
    ));
    if let Some(value) = shipping.line2.as_deref() {
        form.push((
            "payment_intent_data[shipping][address][line2]".into(),
            value.to_owned(),
        ));
    }
    if let Some(value) = shipping.state.as_deref() {
        form.push((
            "payment_intent_data[shipping][address][state]".into(),
            value.to_owned(),
        ));
    }
    if let Some(value) = shipping.postal_code.as_deref() {
        form.push((
            "payment_intent_data[shipping][address][postal_code]".into(),
            value.to_owned(),
        ));
    }
    if let Some(value) = phone {
        form.push((
            "payment_intent_data[shipping][phone]".into(),
            value.to_owned(),
        ));
    }
}

pub struct StripeWebhookVerifier {
    configurations: Arc<dyn StripeWebhookConfigurationRepository>,
    secrets: Arc<dyn IntegrationSecretResolver>,
}

impl StripeWebhookVerifier {
    pub fn new(
        configurations: Arc<dyn StripeWebhookConfigurationRepository>,
        secrets: Arc<dyn IntegrationSecretResolver>,
    ) -> Self {
        Self {
            configurations,
            secrets,
        }
    }
}

#[async_trait]
impl PaymentWebhookVerifier for StripeWebhookVerifier {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn verify(
        &self,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<StripeWebhookEvent, ApplicationError> {
        let raw: Value = serde_json::from_slice(payload).map_err(|_| invalid_webhook())?;
        let envelope: StripeEventEnvelope =
            serde_json::from_value(raw.clone()).map_err(|_| invalid_webhook())?;
        if !valid_stripe_identifier(&envelope.id, "evt_") {
            return Err(invalid_webhook());
        }
        if envelope.account.is_some() {
            return Err(invalid_webhook());
        }
        let configurations = self
            .configurations
            .webhook_configuration(StripeAccountId::from_uuid(provider_account_id))
            .await?;
        if configurations.is_empty() {
            return Err(ApplicationError::Unauthorized);
        }
        let mut verified_account_id = None;
        for configuration in configurations {
            let secret = self
                .secrets
                .resolve(configuration.secret_reference.expose_reference())
                .await?;
            if verify_stripe_signature(signature, payload, secret.expose_secret(), received_at)
                .is_ok()
            {
                verified_account_id = Some(configuration.stripe_account_id);
                break;
            }
        }
        let provider_account_id = verified_account_id.ok_or(ApplicationError::Unauthorized)?;
        let (normalized_event_type, order_id, refund_id, failure_code) =
            map_stripe_event(&envelope)?;
        let object_reference = envelope
            .data
            .as_ref()
            .and_then(|data| data.object.as_ref())
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let provider_payment_intent = envelope
            .data
            .as_ref()
            .and_then(|data| data.object.as_ref())
            .and_then(|object| object.get("payment_intent"))
            .cloned();
        let provider_amount = envelope
            .data
            .as_ref()
            .and_then(|data| data.object.as_ref())
            .and_then(|object| object.get("amount"))
            .cloned();
        Ok(StripeWebhookEvent {
            provider_account_id,
            provider_event_id: envelope.id,
            provider_event_type: envelope.event_type,
            normalized_event_type,
            object_reference: object_reference.clone(),
            order_id,
            refund_id,
            failure_code: failure_code.clone(),
            payload: serde_json::json!({
                "order_id": order_id,
                "refund_id": refund_id,
                "object": object_reference,
                "provider_payment_intent": provider_payment_intent,
                "provider_amount": provider_amount,
                "failure_code": failure_code,
                "stripe_event": raw,
            }),
            verified_at: received_at,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StripeCredentialsWire {
    secret_key: String,
    publishable_key: String,
}

struct StripeCredentials {
    secret_key: SecretString,
    publishable_key: SecretString,
}

impl<'de> Deserialize<'de> for StripeCredentials {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = StripeCredentialsWire::deserialize(deserializer)?;
        Ok(Self {
            secret_key: SecretString::from(value.secret_key),
            publishable_key: SecretString::from(value.publishable_key),
        })
    }
}

#[derive(Deserialize)]
struct StripeObject {
    id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    payment_intent: Option<String>,
}

#[derive(Deserialize)]
struct StripeRefundList {
    #[serde(default)]
    data: Vec<StripeRefundObject>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Deserialize)]
struct StripeRefundObject {
    id: String,
    amount: i64,
    currency: String,
    #[serde(default)]
    payment_intent: Option<String>,
    status: String,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

fn payment_refund_observation(
    refund: StripeRefundObject,
) -> Result<PaymentRefundObservation, ApplicationError> {
    if !valid_stripe_identifier(&refund.id, "re_") || refund.amount <= 0 {
        return Err(stripe_invalid_response());
    }
    if refund
        .payment_intent
        .as_deref()
        .is_some_and(|value| !valid_stripe_identifier(value, "pi_"))
    {
        return Err(stripe_invalid_response());
    }
    let status = match refund.status.as_str() {
        "pending" => PaymentRefundStatus::Pending,
        "requires_action" => PaymentRefundStatus::RequiresAction,
        "succeeded" => PaymentRefundStatus::Succeeded,
        "failed" => PaymentRefundStatus::Failed,
        "canceled" => PaymentRefundStatus::Canceled,
        _ => return Err(stripe_invalid_response()),
    };
    let failure_code = match refund.failure_reason {
        Some(value) if !value.trim().is_empty() && value.chars().count() <= 2000 => Some(value),
        Some(_) => return Err(stripe_invalid_response()),
        None => None,
    };
    let chaos_refund_id = refund
        .metadata
        .get("chaos_refund_id")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil());
    Ok(PaymentRefundObservation {
        provider_reference_id: refund.id,
        amount_minor: refund.amount,
        currency: CurrencyCode::parse(&refund.currency.to_ascii_uppercase())?,
        status,
        failure_code,
        chaos_refund_id,
    })
}

#[derive(Deserialize)]
struct StripeEventEnvelope {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    data: Option<StripeEventData>,
}

#[derive(Deserialize)]
struct StripeEventData {
    #[serde(default)]
    object: Option<Value>,
}

#[derive(Deserialize)]
struct StripeEventObject {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    payment_status: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
    #[serde(default)]
    last_payment_error: Option<StripeFailure>,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

impl StripeEventObject {
    fn failure_code(&self) -> Option<String> {
        self.last_payment_error
            .as_ref()
            .and_then(|error| error.code.clone())
            .or_else(|| self.failure_reason.clone())
    }
}

#[derive(Deserialize)]
struct StripeFailure {
    #[serde(default)]
    code: Option<String>,
}

/// (normalized_event_type, order_id, refund_id, failure_code)
type MappedStripeEvent = (Option<String>, Option<Uuid>, Option<Uuid>, Option<String>);

fn map_stripe_event(event: &StripeEventEnvelope) -> Result<MappedStripeEvent, ApplicationError> {
    let known_wire_event = matches!(
        event.event_type.as_str(),
        "checkout.session.completed"
            | "checkout.session.async_payment_succeeded"
            | "checkout.session.async_payment_failed"
            | "checkout.session.expired"
            | "charge.refunded"
            | "refund.created"
            | "refund.updated"
            | "refund.failed"
    );
    let Some(data) = event.data.as_ref() else {
        return if known_wire_event {
            Err(invalid_webhook())
        } else {
            Ok((None, None, None, None))
        };
    };
    let Some(raw_object) = data.object.as_ref() else {
        return if known_wire_event {
            Err(invalid_webhook())
        } else {
            Ok((None, None, None, None))
        };
    };
    let object: StripeEventObject = match serde_json::from_value(raw_object.clone()) {
        Ok(object) => object,
        Err(_) if known_wire_event => return Err(invalid_webhook()),
        Err(_) => return Ok((None, None, None, None)),
    };
    let (normalized_event_type, object_prefix) = match event.event_type.as_str() {
        "checkout.session.completed"
            if matches!(
                object.payment_status.as_deref(),
                Some("paid" | "no_payment_required")
            ) =>
        {
            (Some("payment.captured"), "cs_")
        }
        // "checkout.session.completed" with payment_status == "unpaid" means
        // an async payment method was selected and the checkout form was
        // submitted, but funds have not settled yet. Wait for the
        // async_payment_succeeded/failed follow-up event instead of
        // transitioning state now — it remains a verified but unnormalized
        // provider event until a later follow-up event arrives.
        "checkout.session.async_payment_succeeded" => (Some("payment.captured"), "cs_"),
        "checkout.session.async_payment_failed" => (Some("payment.failed"), "cs_"),
        "checkout.session.expired" => (Some("payment.expired"), "cs_"),
        // A charge-level notification is the trigger for a provider API
        // reconciliation. It contains the aggregate amount but not the
        // individual Refund objects, so it must never create a local Refund
        // row directly.
        "charge.refunded" => (Some("refund.reconcile"), "ch_"),
        // Refund events created by Chaos carry the order metadata. Dashboard
        // refunds (no chaos_order_id metadata) are correlated later through
        // the PaymentIntent reference — order_id is None for those here.
        "refund.created" | "refund.updated" | "refund.failed" => match object.status.as_deref() {
            Some("succeeded") => (Some("refund.succeeded"), "re_"),
            Some("failed" | "canceled") => (Some("refund.failed"), "re_"),
            Some("pending" | "requires_action") => (Some("refund.pending"), "re_"),
            _ => (None, ""),
        },
        // A verified provider event that Chaos does not act on keeps its raw
        // provider type and is later marked unsupported by the Worker. This
        // prevents endless retries while preserving the payload for support.
        _ => (None, ""),
    };
    if !object_prefix.is_empty() {
        let object_id = object.id.as_deref().ok_or_else(invalid_webhook)?;
        if !valid_stripe_identifier(object_id, object_prefix) {
            return Err(invalid_webhook());
        }
    }
    let order_id = object
        .metadata
        .get("chaos_order_id")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil());
    if normalized_event_type.is_some_and(|event_type| !event_type.starts_with("refund."))
        && order_id.is_none()
    {
        return Err(invalid_webhook());
    }
    let refund_id = object
        .metadata
        .get("chaos_refund_id")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil());
    let failure_code = object.failure_code();
    if failure_code
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 255)
    {
        return Err(invalid_webhook());
    }
    Ok((
        normalized_event_type.map(str::to_owned),
        order_id,
        refund_id,
        failure_code,
    ))
}

fn stripe_headers(
    secret_key: &str,
    idempotency_key: Option<&str>,
) -> Result<HeaderMap, ApplicationError> {
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {secret_key}")).map_err(|_| secret_unavailable())?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(
        "stripe-version",
        HeaderValue::from_static(STRIPE_API_VERSION),
    );
    if let Some(idempotency_key) = idempotency_key {
        headers.insert(
            "idempotency-key",
            HeaderValue::from_str(idempotency_key).map_err(|_| stripe_invalid_response())?,
        );
    }
    Ok(headers)
}

async fn parse_stripe_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApplicationError> {
    let status = response.status();
    if status.is_success() {
        return response
            .json::<T>()
            .await
            .map_err(|_| stripe_invalid_response());
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(ApplicationError::Unavailable {
            service: "stripe",
            source: anyhow::anyhow!("Stripe returned HTTP {status}"),
        })
    } else {
        // Stripe's error body carries the actionable detail (bad param,
        // account misconfiguration); the client only ever sees a generic
        // rejection, so this is the only place that detail is visible.
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(status = %status, body = %body, "Stripe rejected the request");
        Err(ApplicationError::Conflict {
            code: "stripe_request_rejected",
            message: "Stripe rejected the payment operation",
        })
    }
}

fn verify_stripe_signature(
    header: &str,
    payload: &[u8],
    secret: &str,
    received_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for component in header.split(',') {
        let Some((name, value)) = component.trim().split_once('=') else {
            continue;
        };
        match name {
            "t" => timestamp = value.parse::<i64>().ok(),
            "v1" => signatures.push(decode_hex(value).ok_or(ApplicationError::Unauthorized)?),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(ApplicationError::Unauthorized)?;
    if (received_at.unix_timestamp() - timestamp).abs() > WEBHOOK_TOLERANCE_SECONDS {
        return Err(ApplicationError::Unauthorized);
    }
    let mut signed_payload = timestamp.to_string().into_bytes();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(payload);
    let valid = signatures.into_iter().any(|signature| {
        Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map(|mut mac| {
                mac.update(&signed_payload);
                mac.verify_slice(&signature).is_ok()
            })
            .unwrap_or(false)
    });
    if valid {
        Ok(())
    } else {
        Err(ApplicationError::Unauthorized)
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn valid_stripe_identifier(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn provider_network_error(error: reqwest::Error) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "stripe",
        source: anyhow::Error::new(error),
    }
}

fn stripe_invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "stripe",
        source: anyhow::anyhow!("Stripe returned an invalid response"),
    }
}

fn secret_unavailable() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "payment_secret_manager",
        source: anyhow::anyhow!("Payment Provider credentials are unavailable"),
    }
}

fn invalid_webhook() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "payload",
            reason: "must be a valid Stripe webhook event".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::contracts::{
        OrderMetadataContext, PaymentCheckoutDetails, PaymentLineItem, PaymentShippingAddress,
        PaymentShippingOption, StripeWebhookConfiguration,
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::State,
        http::{Request, Response},
        routing::any,
    };
    use chaos_domain::{CurrencyCode, stripe::PaymentSecretReference};

    use super::*;

    const TEST_PROVIDER_ACCOUNT_ID: Uuid = Uuid::from_u128(1);

    fn checkout_details() -> PaymentCheckoutDetails {
        PaymentCheckoutDetails {
            customer_email: Some("buyer@example.com".into()),
            customer_phone: Some("+14155552671".into()),
            shipping_address: Some(PaymentShippingAddress {
                name: "Buyer Example".into(),
                line1: "1 Market Street".into(),
                line2: Some("Suite 100".into()),
                city: "San Francisco".into(),
                state: Some("CA".into()),
                postal_code: Some("94105".into()),
                country_code: "US".into(),
            }),
            line_items: vec![PaymentLineItem {
                name: "T-shirt — Medium / Black".into(),
                sku: Some("TS-M-BLK".into()),
                image_url: Some("https://cdn.example/tshirt-black-m.jpg".into()),
                quantity: 1,
                unit_amount_minor: 1234,
            }],
            shipping_countries: vec!["US".into()],
            shipping_options: vec![PaymentShippingOption {
                service_id: Uuid::from_u128(2),
                code: "standard-us".into(),
                name: "Standard Shipping".into(),
                amount_minor: 199,
                currency: CurrencyCode::parse("USD").unwrap(),
                estimated_min_days: 5,
                estimated_max_days: 8,
            }],
            automatic_tax: true,
        }
    }

    fn order_metadata_context() -> OrderMetadataContext {
        OrderMetadataContext {
            store_id: Uuid::from_u128(3),
            shopper_id: Uuid::from_u128(4),
            channel_id: Uuid::from_u128(5),
            order_number: "W-20260101-ABCDEFGH".into(),
        }
    }

    struct StaticSecrets(HashMap<String, String>);

    #[async_trait]
    impl IntegrationSecretResolver for StaticSecrets {
        async fn resolve(&self, reference: &str) -> Result<SecretString, ApplicationError> {
            self.0
                .get(reference)
                .cloned()
                .map(SecretString::from)
                .ok_or_else(secret_unavailable)
        }
    }

    struct StaticWebhookConfiguration(Vec<PaymentSecretReference>);

    #[async_trait]
    impl StripeWebhookConfigurationRepository for StaticWebhookConfiguration {
        async fn webhook_configuration(
            &self,
            provider_account_id: StripeAccountId,
        ) -> Result<Vec<StripeWebhookConfiguration>, ApplicationError> {
            if provider_account_id.as_uuid() != TEST_PROVIDER_ACCOUNT_ID {
                return Ok(Vec::new());
            }
            Ok(self
                .0
                .iter()
                .cloned()
                .map(|secret_reference| StripeWebhookConfiguration {
                    stripe_account_id: TEST_PROVIDER_ACCOUNT_ID,
                    secret_reference,
                })
                .collect())
        }
    }

    struct RecordedRequest {
        method: String,
        path: String,
        headers: HeaderMap,
        body: String,
    }

    #[derive(Clone)]
    struct MockState(Arc<Mutex<Vec<RecordedRequest>>>);

    async fn stripe_mock(State(state): State<MockState>, request: Request<Body>) -> Response<Body> {
        let method = request.method().to_string();
        let path = request.uri().path().to_owned();
        let headers = request.headers().clone();
        let body = String::from_utf8(
            to_bytes(request.into_body(), 16 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        state.0.lock().unwrap().push(RecordedRequest {
            method: method.clone(),
            path: path.clone(),
            headers,
            body,
        });
        let json = match (method.as_str(), path.as_str()) {
            ("POST", "/v1/payment_intents") => {
                r#"{"id":"pi_created","client_secret":"pi_created_secret_value"}"#
            }
            ("GET", "/v1/payment_intents/pi_created") => {
                r#"{"id":"pi_created","client_secret":"pi_created_secret_value"}"#
            }
            ("GET", "/v1/refunds") => {
                r#"{"object":"list","has_more":false,"data":[
                    {"id":"re_first","amount":2000,"currency":"usd","payment_intent":"pi_created","status":"succeeded","metadata":{}},
                    {"id":"re_canceled","amount":300,"currency":"usd","payment_intent":"pi_created","status":"canceled","failure_reason":"merchant_request","metadata":{}}
                ]}"#
            }
            ("POST", "/v1/refunds") => r#"{"id":"re_created"}"#,
            ("POST", "/v1/checkout/sessions") => {
                r#"{"id":"cs_created","client_secret":"cs_created_secret_value"}"#
            }
            _ => return Response::builder().status(404).body(Body::empty()).unwrap(),
        };
        Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(json))
            .unwrap()
    }

    #[tokio::test]
    async fn stripe_checkout_adapter_executes_payment_over_http() {
        let state = MockState(Arc::new(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(stripe_mock))
                    .with_state(state.clone()),
            )
            .into_future(),
        );
        let reference = PaymentSecretReference::new("credential", "test://stripe").unwrap();
        let secrets = Arc::new(StaticSecrets(HashMap::from([(
            "test://stripe".into(),
            r#"{"secret_key":"sk_test_secret","publishable_key":"pk_test_public"}"#.into(),
        )])));
        let provider = StripeGateway::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let aggregate_id = Uuid::now_v7();
        let created = provider
            .execute(PaymentCommand {
                kind: PaymentCommandKind::CreateCheckoutSession,
                aggregate_id,
                refund_id: None,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "payment-command".into(),
                provider_account_id: TEST_PROVIDER_ACCOUNT_ID,
                credential_secret_reference: reference.expose_reference().into(),
                provider_payment_reference: None,
                checkout_details: Some(checkout_details()),
                return_url: Some("https://shop.example.com/success".into()),
                order_context: order_metadata_context(),
            })
            .await
            .unwrap();
        assert_eq!(created.provider_object_id, "cs_created");
        let action = created.client_action.as_ref().unwrap();
        assert_eq!(action.public_key.expose_secret(), "pk_test_public");
        assert_eq!(
            action.client_token.expose_secret(),
            "cs_created_secret_value"
        );
        {
            let requests = state.0.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, "POST");
            assert_eq!(requests[0].path, "/v1/checkout/sessions");
            assert!(requests[0].headers.get("stripe-account").is_none());
            assert_eq!(requests[0].headers["stripe-version"], STRIPE_API_VERSION);
            assert_eq!(requests[0].headers["idempotency-key"], "payment-command");
            assert_eq!(requests[0].headers[AUTHORIZATION], "Bearer sk_test_secret");
            let checkout_form: HashMap<String, String> =
                url::form_urlencoded::parse(requests[0].body.as_bytes())
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect();
            assert_eq!(checkout_form["ui_mode"], "embedded_page");
            assert_eq!(
                checkout_form["return_url"],
                "https://shop.example.com/success"
            );
            assert_eq!(checkout_form["customer_email"], "buyer@example.com");
            assert_eq!(checkout_form["phone_number_collection[enabled]"], "true");
            assert_eq!(checkout_form["billing_address_collection"], "required");
            assert_eq!(checkout_form["allow_promotion_codes"], "true");
            assert_eq!(
                checkout_form["payment_intent_data[receipt_email]"],
                "buyer@example.com"
            );
            assert_eq!(
                checkout_form["payment_intent_data[shipping][phone]"],
                "+14155552671"
            );
            assert_eq!(
                checkout_form["payment_intent_data[shipping][address][country]"],
                "US"
            );
            assert_eq!(
                checkout_form["metadata[chaos_order_id]"],
                aggregate_id.to_string()
            );
            drop(checkout_form);
        }
        let refund_order_id = Uuid::now_v7();
        let refund_id = Uuid::now_v7();
        let refunded = provider
            .execute(PaymentCommand {
                kind: PaymentCommandKind::CreateRefund,
                aggregate_id: refund_order_id,
                refund_id: Some(refund_id),
                amount_minor: 500,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "refund-command".into(),
                provider_account_id: TEST_PROVIDER_ACCOUNT_ID,
                credential_secret_reference: reference.expose_reference().into(),
                provider_payment_reference: Some("pi_created".into()),
                checkout_details: None,
                return_url: None,
                order_context: order_metadata_context(),
            })
            .await
            .unwrap();
        assert_eq!(refunded.provider_object_id, "re_created");
        let requests = state.0.lock().unwrap();
        let refund_request = requests.last().unwrap();
        assert_eq!(refund_request.method, "POST");
        assert_eq!(refund_request.path, "/v1/refunds");
        let refund_form: HashMap<_, _> =
            url::form_urlencoded::parse(refund_request.body.as_bytes()).collect();
        assert_eq!(refund_form["payment_intent"], "pi_created");
        assert_eq!(refund_form["amount"], "500");
        assert_eq!(
            refund_form["metadata[chaos_order_id]"],
            refund_order_id.to_string()
        );
        assert_eq!(
            refund_form["metadata[chaos_refund_id]"],
            refund_id.to_string()
        );
        assert_eq!(refund_request.headers["idempotency-key"], "refund-command");
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn stripe_gateway_lists_succeeded_and_canceled_refunds() {
        let state = MockState(Arc::new(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(stripe_mock))
                    .with_state(state.clone()),
            )
            .into_future(),
        );
        let reference = PaymentSecretReference::new("credential", "test://stripe").unwrap();
        let secrets = Arc::new(StaticSecrets(HashMap::from([(
            "test://stripe".into(),
            r#"{"secret_key":"sk_test_secret","publishable_key":"pk_test_public"}"#.into(),
        )])));
        let provider = StripeGateway::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let refunds = provider
            .list_refunds(reference.expose_reference(), "pi_created")
            .await
            .unwrap();
        assert_eq!(refunds.len(), 2);
        assert_eq!(refunds[0].provider_reference_id, "re_first");
        assert_eq!(refunds[0].status, PaymentRefundStatus::Succeeded);
        assert_eq!(refunds[1].provider_reference_id, "re_canceled");
        assert_eq!(refunds[1].status, PaymentRefundStatus::Canceled);
        assert_eq!(refunds[1].failure_code.as_deref(), Some("merchant_request"));
        server.abort();
    }

    #[tokio::test]
    async fn stripe_checkout_adapter_omits_customer_email_when_not_yet_known() {
        let state = MockState(Arc::new(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(stripe_mock))
                    .with_state(state.clone()),
            )
            .into_future(),
        );
        let reference = PaymentSecretReference::new("credential", "test://stripe").unwrap();
        let secrets = Arc::new(StaticSecrets(HashMap::from([(
            "test://stripe".into(),
            r#"{"secret_key":"sk_test_secret","publishable_key":"pk_test_public"}"#.into(),
        )])));
        let provider = StripeGateway::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let mut details = checkout_details();
        details.customer_email = None;
        provider
            .execute(PaymentCommand {
                kind: PaymentCommandKind::CreateCheckoutSession,
                aggregate_id: Uuid::now_v7(),
                refund_id: None,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "payment-command-no-email".into(),
                provider_account_id: TEST_PROVIDER_ACCOUNT_ID,
                credential_secret_reference: reference.expose_reference().into(),
                provider_payment_reference: None,
                checkout_details: Some(details),
                return_url: Some("https://shop.example.com/success".into()),
                order_context: order_metadata_context(),
            })
            .await
            .unwrap();
        let requests = state.0.lock().unwrap();
        let checkout_form: HashMap<String, String> =
            url::form_urlencoded::parse(requests[0].body.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
        assert!(!checkout_form.contains_key("customer_email"));
        assert!(!checkout_form.contains_key("payment_intent_data[receipt_email]"));
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn stripe_checkout_adapter_creates_an_embedded_session_and_returns_its_client_secret() {
        let state = MockState(Arc::new(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(stripe_mock))
                    .with_state(state.clone()),
            )
            .into_future(),
        );
        let reference = PaymentSecretReference::new("credential", "test://stripe").unwrap();
        let secrets = Arc::new(StaticSecrets(HashMap::from([(
            "test://stripe".into(),
            r#"{"secret_key":"sk_test_secret","publishable_key":"pk_test_public"}"#.into(),
        )])));
        let provider = StripeGateway::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        assert_eq!(PaymentProvider::name(&provider), "stripe");
        let aggregate_id = Uuid::now_v7();
        let created = provider
            .execute(PaymentCommand {
                kind: PaymentCommandKind::CreateCheckoutSession,
                aggregate_id,
                refund_id: None,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "checkout-command".into(),
                provider_account_id: TEST_PROVIDER_ACCOUNT_ID,
                credential_secret_reference: reference.expose_reference().into(),
                provider_payment_reference: None,
                checkout_details: Some(checkout_details()),
                return_url: Some("https://shop.example.com/success".into()),
                order_context: order_metadata_context(),
            })
            .await
            .unwrap();
        assert_eq!(created.provider_object_id, "cs_created");
        let action = created.client_action.as_ref().unwrap();
        assert_eq!(action.kind, "mount_embedded_checkout");
        assert_eq!(
            action.client_token.expose_secret(),
            "cs_created_secret_value"
        );
        let requests = state.0.lock().unwrap();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/v1/checkout/sessions");
        assert!(requests[0].headers.get("stripe-account").is_none());
        let form: HashMap<_, _> =
            url::form_urlencoded::parse(requests[0].body.as_bytes()).collect();
        assert_eq!(form["mode"], "payment");
        assert_eq!(form["ui_mode"], "embedded_page");
        assert_eq!(form["return_url"], "https://shop.example.com/success");
        assert_eq!(form["customer_email"], "buyer@example.com");
        assert_eq!(form["phone_number_collection[enabled]"], "true");
        assert_eq!(form["billing_address_collection"], "required");
        assert_eq!(form["allow_promotion_codes"], "true");
        assert_eq!(
            form["payment_intent_data[shipping][address][country]"],
            "US"
        );
        assert_eq!(form["line_items[0][quantity]"], "1");
        assert_eq!(form["line_items[0][price_data][currency]"], "usd");
        assert_eq!(form["line_items[0][price_data][unit_amount]"], "1234");
        assert_eq!(
            form["line_items[0][price_data][product_data][images][0]"],
            "https://cdn.example/tshirt-black-m.jpg"
        );
        assert_eq!(form["metadata[chaos_order_id]"], aggregate_id.to_string());
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn stripe_checkout_adapter_rejects_creation_without_return_url() {
        let reference = PaymentSecretReference::new("credential", "test://stripe").unwrap();
        let secrets = Arc::new(StaticSecrets(HashMap::from([(
            "test://stripe".into(),
            r#"{"secret_key":"sk_test_secret","publishable_key":"pk_test_public"}"#.into(),
        )])));
        let provider = StripeGateway::new(
            "http://127.0.0.1:1/".parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let result = provider
            .execute(PaymentCommand {
                kind: PaymentCommandKind::CreateCheckoutSession,
                aggregate_id: Uuid::now_v7(),
                refund_id: None,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "checkout-command".into(),
                provider_account_id: TEST_PROVIDER_ACCOUNT_ID,
                credential_secret_reference: reference.expose_reference().into(),
                provider_payment_reference: None,
                checkout_details: Some(checkout_details()),
                return_url: None,
                order_context: order_metadata_context(),
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stripe_checkout_webhook_routes_by_provider_account_id() {
        let active_reference =
            PaymentSecretReference::new("webhook", "test://webhook-active").unwrap();
        let previous_reference =
            PaymentSecretReference::new("webhook", "test://webhook-previous").unwrap();
        let verifier = StripeWebhookVerifier::new(
            Arc::new(StaticWebhookConfiguration(vec![
                active_reference,
                previous_reference,
            ])),
            Arc::new(StaticSecrets(HashMap::from([
                ("test://webhook-active".into(), "whsec_active_value".into()),
                (
                    "test://webhook-previous".into(),
                    "whsec_previous_value".into(),
                ),
            ]))),
        );
        let aggregate_id = Uuid::now_v7();
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": "evt_1",
            "type": "checkout.session.completed",
            "data": {"object": {
                "id": "cs_created",
                "payment_status": "paid",
                "metadata": {"chaos_order_id": aggregate_id}
            }}
        }))
        .unwrap();
        let received_at = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let signed = format!(
            "{}.{}",
            received_at.unix_timestamp(),
            String::from_utf8_lossy(&payload)
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_previous_value").unwrap();
        mac.update(signed.as_bytes());
        let signature = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let event = verifier
            .verify(
                TEST_PROVIDER_ACCOUNT_ID,
                &format!("t={},v1={signature}", received_at.unix_timestamp()),
                &payload,
                received_at,
            )
            .await
            .unwrap();
        assert_eq!(
            event.normalized_event_type.as_deref(),
            Some("payment.captured")
        );
        assert_eq!(event.object_reference.as_deref(), Some("cs_created"));
        assert_eq!(event.provider_account_id, TEST_PROVIDER_ACCOUNT_ID);
        assert_eq!(event.order_id, Some(aggregate_id));
        assert_eq!(event.payload["order_id"], aggregate_id.to_string());

        assert!(
            verifier
                .verify(
                    Uuid::from_u128(3),
                    &format!("t={},v1={signature}", received_at.unix_timestamp()),
                    &payload,
                    received_at,
                )
                .await
                .is_err()
        );
        assert!(
            verifier
                .verify(
                    TEST_PROVIDER_ACCOUNT_ID,
                    &format!("t={},v1={signature}", received_at.unix_timestamp()),
                    &payload,
                    received_at + time::Duration::minutes(6),
                )
                .await
                .is_err()
        );

        let connect_payload = serde_json::to_vec(&serde_json::json!({
            "id": "evt_2",
            "type": "checkout.session.completed",
            "account": "acct_connect",
            "data": {"object": {
                "id": "cs_created",
                "payment_status": "paid",
                "metadata": {"chaos_order_id": aggregate_id}
            }}
        }))
        .unwrap();
        let connect_signed = format!(
            "{}.{}",
            received_at.unix_timestamp(),
            String::from_utf8_lossy(&connect_payload)
        );
        let mut connect_mac = Hmac::<Sha256>::new_from_slice(b"whsec_active_value").unwrap();
        connect_mac.update(connect_signed.as_bytes());
        let connect_signature = connect_mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(
            verifier
                .verify(
                    TEST_PROVIDER_ACCOUNT_ID,
                    &format!("t={},v1={connect_signature}", received_at.unix_timestamp()),
                    &connect_payload,
                    received_at,
                )
                .await
                .is_err()
        );
    }

    fn checkout_session_event(
        event_type: &str,
        payment_status: Option<&str>,
        aggregate_id: Uuid,
    ) -> StripeEventEnvelope {
        let mut object = serde_json::json!({
            "id": "cs_created",
            "metadata": {"chaos_order_id": aggregate_id}
        });
        if let Some(status) = payment_status {
            object["payment_status"] = serde_json::Value::String(status.into());
        }
        serde_json::from_value(serde_json::json!({
            "id": "evt_1",
            "type": event_type,
            "data": {"object": object}
        }))
        .unwrap()
    }

    #[test]
    fn checkout_session_completed_paid_captures_immediately() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.completed", Some("paid"), aggregate_id);
        let (event_type, order_id, refund_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("payment.captured"));
        assert_eq!(order_id, Some(aggregate_id));
        assert_eq!(refund_id, None);
        assert_eq!(failure_code, None);
    }

    #[test]
    fn checkout_session_completed_no_payment_required_captures_immediately() {
        let aggregate_id = Uuid::now_v7();
        let event = checkout_session_event(
            "checkout.session.completed",
            Some("no_payment_required"),
            aggregate_id,
        );
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("payment.captured"));
    }

    #[test]
    fn checkout_session_completed_unpaid_is_ignored_pending_the_async_follow_up() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.completed", Some("unpaid"), aggregate_id);
        let (event_type, order_id, refund_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, None);
        assert_eq!(order_id, Some(aggregate_id));
        assert_eq!(refund_id, None);
        assert_eq!(failure_code, None);
    }

    #[test]
    fn authenticated_unhandled_events_have_no_normalized_type() {
        let aggregate_id = Uuid::now_v7();
        let event = checkout_session_event("charge.succeeded", None, aggregate_id);
        let (event_type, order_id, refund_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, None);
        assert_eq!(order_id, Some(aggregate_id));
        assert_eq!(refund_id, None);
        assert_eq!(failure_code, None);
    }

    #[test]
    fn authenticated_unhandled_event_without_chaos_metadata_is_still_accepted() {
        let event: StripeEventEnvelope = serde_json::from_value(serde_json::json!({
            "id": "evt_future",
            "type": "future.payment.event",
            "data": {"object": {"metadata": {}}}
        }))
        .unwrap();
        let (event_type, order_id, refund_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, None);
        assert_eq!(order_id, None);
        assert_eq!(refund_id, None);
        assert_eq!(failure_code, None);
    }

    #[test]
    fn checkout_session_async_payment_succeeded_captures() {
        let aggregate_id = Uuid::now_v7();
        let event = checkout_session_event(
            "checkout.session.async_payment_succeeded",
            None,
            aggregate_id,
        );
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("payment.captured"));
    }

    #[test]
    fn checkout_session_async_payment_failed_fails() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.async_payment_failed", None, aggregate_id);
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("payment.failed"));
    }

    #[test]
    fn checkout_session_expired_expires_payment() {
        let aggregate_id = Uuid::now_v7();
        let event = checkout_session_event("checkout.session.expired", None, aggregate_id);
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("payment.expired"));
    }

    fn refund_event(
        event_type: &str,
        status: &str,
        order_id: Uuid,
        refund_id: Uuid,
    ) -> StripeEventEnvelope {
        serde_json::from_value(serde_json::json!({
            "id": "evt_refund",
            "type": event_type,
            "data": {"object": {
                "id": "re_created",
                "status": status,
                "metadata": {"chaos_order_id": order_id, "chaos_refund_id": refund_id}
            }}
        }))
        .unwrap()
    }

    #[test]
    fn refund_created_succeeded_is_applied_to_the_local_refund() {
        let order_id = Uuid::now_v7();
        let refund_id = Uuid::now_v7();
        let event = refund_event("refund.created", "succeeded", order_id, refund_id);
        let (event_type, resolved_order_id, resolved_refund_id, failure_code) =
            map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("refund.succeeded"));
        assert_eq!(resolved_order_id, Some(order_id));
        assert_eq!(resolved_refund_id, Some(refund_id));
        assert_eq!(failure_code, None);
    }

    #[test]
    fn refund_updated_failed_is_applied_to_the_local_refund() {
        let order_id = Uuid::now_v7();
        let refund_id = Uuid::now_v7();
        let event = refund_event("refund.updated", "failed", order_id, refund_id);
        let (event_type, resolved_order_id, resolved_refund_id, failure_code) =
            map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("refund.failed"));
        assert_eq!(resolved_order_id, Some(order_id));
        assert_eq!(resolved_refund_id, Some(refund_id));
        assert_eq!(failure_code, None);
    }

    #[test]
    fn refund_failed_canceled_is_handled_as_a_refund_failure() {
        let order_id = Uuid::now_v7();
        let refund_id = Uuid::now_v7();
        let event = refund_event("refund.failed", "canceled", order_id, refund_id);
        let (event_type, resolved_order_id, resolved_refund_id, failure_code) =
            map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("refund.failed"));
        assert_eq!(resolved_order_id, Some(order_id));
        assert_eq!(resolved_refund_id, Some(refund_id));
        assert_eq!(failure_code, None);
    }

    #[test]
    fn charge_refunded_requests_refund_reconciliation() {
        let event: StripeEventEnvelope = serde_json::from_value(serde_json::json!({
            "id": "evt_charge_refunded",
            "type": "charge.refunded",
            "data": {"object": {
                "id": "ch_refunded",
                "payment_intent": "pi_created",
                "status": "succeeded"
            }}
        }))
        .unwrap();
        let (event_type, order_id, refund_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type.as_deref(), Some("refund.reconcile"));
        assert_eq!(order_id, None);
        assert_eq!(refund_id, None);
        assert_eq!(failure_code, None);
    }
}
