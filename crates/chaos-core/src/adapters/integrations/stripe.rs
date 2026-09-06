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
                kind: "stripe_checkout_embedded",
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
#[path = "stripe_tests.rs"]
mod tests;
