use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        PaymentClientAction, PaymentProvider, PaymentProviderOnboarding, PaymentProviderReadiness,
        PaymentSecretResolver, PaymentWebhookConfigurationRepository, PaymentWebhookVerifier,
        ProviderClientActionCommand, ProviderCommand, ProviderCommandResult, VerifiedWebhookEvent,
    },
};
use chaos_domain::payments::PaymentSecretReference;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

const STRIPE_API_VERSION: &str = "2026-02-25.clover";
const WEBHOOK_TOLERANCE_SECONDS: i64 = 300;
const STRIPE_PLATFORM_ACCOUNT_PREFIX: &str = "platform:";

/// HTTP plumbing shared by every Stripe-backed `PaymentProvider` adapter:
/// credential resolution, URL joining, and authenticated form POST/GET.
struct StripeHttp {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn PaymentSecretResolver>,
}

impl StripeHttp {
    fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn PaymentSecretResolver>,
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

    async fn credentials(
        &self,
        reference: &PaymentSecretReference,
    ) -> Result<StripeCredentials, ApplicationError> {
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
        connected_account: &str,
        idempotency_key: &str,
        form: &[(String, String)],
    ) -> Result<StripeObject, ApplicationError> {
        let response = self
            .client
            .post(self.endpoint(path)?)
            .headers(stripe_headers(
                credentials.secret_key.expose_secret(),
                connected_account,
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
        connected_account: &str,
        provider_reference: &str,
        expected_prefix: &str,
    ) -> Result<StripeObject, ApplicationError> {
        if !valid_stripe_identifier(provider_reference, expected_prefix) {
            return Err(provider_invalid_response());
        }
        let response = self
            .client
            .get(self.endpoint(&format!("{path_prefix}{provider_reference}"))?)
            .headers(stripe_headers(
                credentials.secret_key.expose_secret(),
                connected_account,
                None,
            )?)
            .send()
            .await
            .map_err(provider_network_error)?;
        parse_stripe_response(response).await
    }

    async fn get_account(
        &self,
        secret_key: &str,
        external_account_reference: &str,
    ) -> Result<StripeAccount, ApplicationError> {
        let path = if is_platform_account(external_account_reference) {
            "v1/account".to_owned()
        } else if valid_stripe_identifier(external_account_reference, "acct_") {
            format!("v1/accounts/{external_account_reference}")
        } else {
            return Err(provider_invalid_response());
        };
        let response = self
            .client
            .get(self.endpoint(&path)?)
            .headers(stripe_platform_headers(secret_key)?)
            .send()
            .await
            .map_err(provider_network_error)?;
        parse_stripe_account_response(response).await
    }
}

pub struct StripePaymentProvider {
    http: StripeHttp,
}

impl StripePaymentProvider {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn PaymentSecretResolver>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            http: StripeHttp::new(api_base_url, timeout, secrets)?,
        })
    }

    async fn retrieve_payment_intent(
        &self,
        credentials: &StripeCredentials,
        connected_account: &str,
        provider_reference: &str,
    ) -> Result<StripeObject, ApplicationError> {
        self.http
            .retrieve_object(
                "v1/payment_intents/",
                credentials,
                connected_account,
                provider_reference,
                "pi_",
            )
            .await
    }
}

#[async_trait]
impl PaymentProvider for StripePaymentProvider {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError> {
        let credentials = self
            .http
            .credentials(&command.credential_secret_reference)
            .await?;
        let (object, expected_prefix) = if command.event_type == "payment.create_requested" {
            self.http
                .send_form(
                    "v1/payment_intents",
                    &credentials,
                    &command.external_account_reference,
                    &command.idempotency_key,
                    &[
                        ("amount".into(), command.amount_minor.to_string()),
                        (
                            "currency".into(),
                            command.currency.as_str().to_ascii_lowercase(),
                        ),
                        ("automatic_payment_methods[enabled]".into(), "true".into()),
                        (
                            "metadata[chaos_payment_attempt_id]".into(),
                            command.aggregate_id.to_string(),
                        ),
                    ],
                )
                .await
                .map(|object| (object, "pi_"))?
        } else if command.event_type == "refund.create_requested" {
            let payment_intent = command
                .payment_provider_reference
                .filter(|value| valid_stripe_identifier(value, "pi_"))
                .ok_or_else(provider_invalid_response)?;
            self.http
                .send_form(
                    "v1/refunds",
                    &credentials,
                    &command.external_account_reference,
                    &command.idempotency_key,
                    &[
                        ("payment_intent".into(), payment_intent),
                        ("amount".into(), command.amount_minor.to_string()),
                        (
                            "metadata[chaos_refund_id]".into(),
                            command.aggregate_id.to_string(),
                        ),
                    ],
                )
                .await
                .map(|object| (object, "re_"))?
        } else {
            return Err(provider_invalid_response());
        };
        if !valid_stripe_identifier(&object.id, expected_prefix) {
            return Err(provider_invalid_response());
        }
        Ok(ProviderCommandResult {
            provider_reference: object.id,
        })
    }

    async fn client_action(
        &self,
        command: ProviderClientActionCommand,
    ) -> Result<PaymentClientAction, ApplicationError> {
        let credentials = self
            .http
            .credentials(&command.credential_secret_reference)
            .await?;
        let object = self
            .retrieve_payment_intent(
                &credentials,
                &command.external_account_reference,
                &command.provider_reference,
            )
            .await?;
        let client_secret = object.client_secret.ok_or_else(provider_invalid_response)?;
        Ok(PaymentClientAction {
            provider: "stripe".into(),
            kind: "confirm_payment",
            public_key: credentials.publishable_key,
            client_token: SecretString::from(client_secret),
            account_reference: command.external_account_reference,
        })
    }
}

#[async_trait]
impl PaymentProviderOnboarding for StripePaymentProvider {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn check_readiness(
        &self,
        external_account_reference: &str,
        credential_secret_reference: &PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError> {
        stripe_account_readiness(
            &self.http,
            external_account_reference,
            credential_secret_reference,
            checked_at,
        )
        .await
    }
}

/// Stripe account readiness shared by every Stripe-backed adapter. The
/// `platform:...` reference uses the account owning the API key; an `acct_...`
/// reference uses Stripe Connect.
async fn stripe_account_readiness(
    http: &StripeHttp,
    external_account_reference: &str,
    credential_secret_reference: &PaymentSecretReference,
    checked_at: OffsetDateTime,
) -> Result<PaymentProviderReadiness, ApplicationError> {
    if !is_platform_account(external_account_reference)
        && !valid_stripe_identifier(external_account_reference, "acct_")
    {
        return Err(provider_invalid_response());
    }
    let credentials = http.credentials(credential_secret_reference).await?;
    let account = http
        .get_account(
            credentials.secret_key.expose_secret(),
            external_account_reference,
        )
        .await?;
    let connected = !is_platform_account(external_account_reference);
    if connected && account.id != external_account_reference {
        return Err(provider_invalid_response());
    }

    let card_payments = account.capabilities.card_payments.as_deref();
    let fee_payer = account.controller.fees.payer.as_deref();
    let losses_payer = account.controller.losses.payments.as_deref();
    let requirements_due =
        account.requirements.currently_due.len() + account.requirements.past_due.len();
    let mut blocker_codes = Vec::new();
    if !account.charges_enabled {
        blocker_codes.push("charges_disabled".into());
    }
    if !account.payouts_enabled {
        blocker_codes.push("payouts_disabled".into());
    }
    if !account.details_submitted {
        blocker_codes.push("details_incomplete".into());
    }
    if card_payments != Some("active") {
        blocker_codes.push("card_payments_inactive".into());
    }
    if requirements_due != 0 || account.requirements.disabled_reason.is_some() {
        blocker_codes.push("requirements_due".into());
    }
    if connected && fee_payer != Some("account") {
        blocker_codes.push("fee_payer_mismatch".into());
    }
    if connected && losses_payer != Some("stripe") {
        blocker_codes.push("loss_liability_mismatch".into());
    }

    let ready = blocker_codes.is_empty();
    let configuration = serde_json::json!({
        "account_reference": account.id,
        "ready": ready,
        "blocker_codes": &blocker_codes,
        "accepts_payments": account.charges_enabled,
        "supports_payouts": account.payouts_enabled,
        "details_submitted": account.details_submitted,
        "card_payments": card_payments,
        "fee_payer": fee_payer,
        "losses_payer": losses_payer,
        "requirements_due": requirements_due,
        "disabled_reason": account.requirements.disabled_reason,
    });
    Ok(PaymentProviderReadiness {
        ready,
        blocker_codes,
        configuration,
        checked_at,
    })
}

pub struct StripeCheckoutPaymentProvider {
    http: StripeHttp,
}

impl StripeCheckoutPaymentProvider {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<dyn PaymentSecretResolver>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            http: StripeHttp::new(api_base_url, timeout, secrets)?,
        })
    }
}

#[async_trait]
impl PaymentProvider for StripeCheckoutPaymentProvider {
    fn name(&self) -> &'static str {
        "stripe_checkout"
    }

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError> {
        if command.event_type != "payment.create_requested" {
            return Err(provider_invalid_response());
        }
        let credentials = self
            .http
            .credentials(&command.credential_secret_reference)
            .await?;
        let return_url = command
            .return_url
            .as_deref()
            .ok_or_else(provider_invalid_response)?;
        let object = self
            .http
            .send_form(
                "v1/checkout/sessions",
                &credentials,
                &command.external_account_reference,
                &command.idempotency_key,
                &[
                    ("mode".into(), "payment".into()),
                    ("ui_mode".into(), "embedded_page".into()),
                    ("return_url".into(), return_url.into()),
                    ("line_items[0][quantity]".into(), "1".into()),
                    (
                        "line_items[0][price_data][currency]".into(),
                        command.currency.as_str().to_ascii_lowercase(),
                    ),
                    (
                        "line_items[0][price_data][unit_amount]".into(),
                        command.amount_minor.to_string(),
                    ),
                    (
                        "line_items[0][price_data][product_data][name]".into(),
                        "Order total".into(),
                    ),
                    (
                        "metadata[chaos_payment_attempt_id]".into(),
                        command.aggregate_id.to_string(),
                    ),
                ],
            )
            .await?;
        if !valid_stripe_identifier(&object.id, "cs_") {
            return Err(provider_invalid_response());
        }
        Ok(ProviderCommandResult {
            provider_reference: object.id,
        })
    }

    async fn client_action(
        &self,
        command: ProviderClientActionCommand,
    ) -> Result<PaymentClientAction, ApplicationError> {
        let credentials = self
            .http
            .credentials(&command.credential_secret_reference)
            .await?;
        let object = self
            .http
            .retrieve_object(
                "v1/checkout/sessions/",
                &credentials,
                &command.external_account_reference,
                &command.provider_reference,
                "cs_",
            )
            .await?;
        let client_secret = object.client_secret.ok_or_else(provider_invalid_response)?;
        Ok(PaymentClientAction {
            provider: "stripe_checkout".into(),
            kind: "mount_embedded_checkout",
            public_key: credentials.publishable_key,
            client_token: SecretString::from(client_secret),
            account_reference: command.external_account_reference,
        })
    }
}

#[async_trait]
impl PaymentProviderOnboarding for StripeCheckoutPaymentProvider {
    fn name(&self) -> &'static str {
        "stripe_checkout"
    }

    async fn check_readiness(
        &self,
        external_account_reference: &str,
        credential_secret_reference: &PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError> {
        stripe_account_readiness(
            &self.http,
            external_account_reference,
            credential_secret_reference,
            checked_at,
        )
        .await
    }
}

pub struct StripeWebhookVerifier {
    provider: &'static str,
    configurations: Arc<dyn PaymentWebhookConfigurationRepository>,
    secrets: Arc<dyn PaymentSecretResolver>,
}

impl StripeWebhookVerifier {
    pub fn new(
        configurations: Arc<dyn PaymentWebhookConfigurationRepository>,
        secrets: Arc<dyn PaymentSecretResolver>,
    ) -> Self {
        Self {
            provider: "stripe",
            configurations,
            secrets,
        }
    }

    pub fn for_provider(
        provider: &'static str,
        configurations: Arc<dyn PaymentWebhookConfigurationRepository>,
        secrets: Arc<dyn PaymentSecretResolver>,
    ) -> Self {
        Self {
            provider,
            configurations,
            secrets,
        }
    }
}

#[async_trait]
impl PaymentWebhookVerifier for StripeWebhookVerifier {
    fn name(&self) -> &'static str {
        self.provider
    }

    async fn verify(
        &self,
        provider: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError> {
        if provider != self.name() {
            return Err(ApplicationError::Unauthorized);
        }
        let raw: Value = serde_json::from_slice(payload).map_err(|_| invalid_webhook())?;
        let envelope: StripeEventEnvelope =
            serde_json::from_value(raw.clone()).map_err(|_| invalid_webhook())?;
        if !valid_stripe_identifier(&envelope.id, "evt_") {
            return Err(invalid_webhook());
        }
        if envelope
            .account
            .as_deref()
            .is_some_and(|account| !valid_stripe_identifier(account, "acct_"))
        {
            return Err(invalid_webhook());
        }
        let configurations = self
            .configurations
            .webhook_configurations(provider, envelope.account.as_deref())
            .await?;
        if configurations.is_empty() {
            return Err(ApplicationError::Unauthorized);
        }
        let mut external_account_reference = None;
        for configuration in configurations {
            let secret = self
                .secrets
                .resolve(&configuration.secret_reference)
                .await?;
            if verify_stripe_signature(signature, payload, secret.expose_secret(), received_at)
                .is_ok()
            {
                external_account_reference = Some(configuration.external_account_reference);
                break;
            }
        }
        let external_account_reference =
            external_account_reference.ok_or(ApplicationError::Unauthorized)?;
        let (event_type, aggregate_id, failure_code) = map_stripe_event(&envelope)?;
        let object_reference = envelope.data.object.id.clone();
        Ok(VerifiedWebhookEvent {
            provider: provider.into(),
            provider_event_id: envelope.id,
            event_type,
            external_account_reference,
            object_reference: object_reference.clone(),
            failure_code: failure_code.clone(),
            payload: serde_json::json!({
                "aggregate_id": aggregate_id,
                "object": object_reference,
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
}

#[derive(Deserialize)]
struct StripeAccount {
    id: String,
    #[serde(default)]
    charges_enabled: bool,
    #[serde(default)]
    payouts_enabled: bool,
    #[serde(default)]
    details_submitted: bool,
    #[serde(default)]
    capabilities: StripeAccountCapabilities,
    #[serde(default)]
    controller: StripeAccountController,
    #[serde(default)]
    requirements: StripeAccountRequirements,
}

#[derive(Default, Deserialize)]
struct StripeAccountCapabilities {
    #[serde(default)]
    card_payments: Option<String>,
}

#[derive(Default, Deserialize)]
struct StripeAccountController {
    #[serde(default)]
    fees: StripeAccountFees,
    #[serde(default)]
    losses: StripeAccountLosses,
}

#[derive(Default, Deserialize)]
struct StripeAccountFees {
    #[serde(default)]
    payer: Option<String>,
}

#[derive(Default, Deserialize)]
struct StripeAccountLosses {
    #[serde(default)]
    payments: Option<String>,
}

#[derive(Default, Deserialize)]
struct StripeAccountRequirements {
    #[serde(default)]
    currently_due: Vec<Value>,
    #[serde(default)]
    past_due: Vec<Value>,
    #[serde(default)]
    disabled_reason: Option<String>,
}

#[derive(Deserialize)]
struct StripeEventEnvelope {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    account: Option<String>,
    data: StripeEventData,
}

#[derive(Deserialize)]
struct StripeEventData {
    object: StripeEventObject,
}

#[derive(Deserialize)]
struct StripeEventObject {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    payment_status: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
    #[serde(default)]
    last_payment_error: Option<StripeFailure>,
    #[serde(default)]
    failure_reason: Option<String>,
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

fn map_stripe_event(
    event: &StripeEventEnvelope,
) -> Result<(String, Uuid, Option<String>), ApplicationError> {
    let (event_type, metadata_key, object_prefix) = match event.event_type.as_str() {
        "payment_intent.amount_capturable_updated" => {
            ("payment.authorized", "chaos_payment_attempt_id", "pi_")
        }
        "payment_intent.succeeded" => ("payment.captured", "chaos_payment_attempt_id", "pi_"),
        "payment_intent.payment_failed" => ("payment.failed", "chaos_payment_attempt_id", "pi_"),
        "payment_intent.canceled" => ("payment.cancelled", "chaos_payment_attempt_id", "pi_"),
        "refund.created" | "refund.updated"
            if event.data.object.status.as_deref() == Some("succeeded") =>
        {
            ("refund.succeeded", "chaos_refund_id", "re_")
        }
        "refund.failed" => ("refund.failed", "chaos_refund_id", "re_"),
        "refund.updated"
            if matches!(
                event.data.object.status.as_deref(),
                Some("failed" | "canceled")
            ) =>
        {
            ("refund.failed", "chaos_refund_id", "re_")
        }
        "checkout.session.completed"
            if matches!(
                event.data.object.payment_status.as_deref(),
                Some("paid" | "no_payment_required")
            ) =>
        {
            ("payment.captured", "chaos_payment_attempt_id", "cs_")
        }
        // "checkout.session.completed" with payment_status == "unpaid" means
        // an async payment method was selected and the checkout form was
        // submitted, but funds have not settled yet. Wait for the
        // async_payment_succeeded/failed follow-up event instead of
        // transitioning state now — falls through to the ignored default.
        "checkout.session.async_payment_succeeded" => {
            ("payment.captured", "chaos_payment_attempt_id", "cs_")
        }
        "checkout.session.async_payment_failed" => {
            ("payment.failed", "chaos_payment_attempt_id", "cs_")
        }
        "checkout.session.expired" => ("payment.cancelled", "chaos_payment_attempt_id", "cs_"),
        _ => return Err(ignored_webhook()),
    };
    if !valid_stripe_identifier(&event.data.object.id, object_prefix) {
        return Err(invalid_webhook());
    }
    let aggregate_id = event
        .data
        .object
        .metadata
        .get(metadata_key)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(invalid_webhook)?;
    let failure_code = event.data.object.failure_code();
    if failure_code
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 255)
    {
        return Err(invalid_webhook());
    }
    Ok((event_type.into(), aggregate_id, failure_code))
}

fn stripe_headers(
    secret_key: &str,
    connected_account: &str,
    idempotency_key: Option<&str>,
) -> Result<HeaderMap, ApplicationError> {
    if !is_platform_account(connected_account)
        && !valid_stripe_identifier(connected_account, "acct_")
    {
        return Err(provider_invalid_response());
    }
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {secret_key}")).map_err(|_| secret_unavailable())?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    if !is_platform_account(connected_account) {
        headers.insert(
            "stripe-account",
            HeaderValue::from_str(connected_account).map_err(|_| provider_invalid_response())?,
        );
    }
    headers.insert(
        "stripe-version",
        HeaderValue::from_static(STRIPE_API_VERSION),
    );
    if let Some(idempotency_key) = idempotency_key {
        headers.insert(
            "idempotency-key",
            HeaderValue::from_str(idempotency_key).map_err(|_| provider_invalid_response())?,
        );
    }
    Ok(headers)
}

fn stripe_platform_headers(secret_key: &str) -> Result<HeaderMap, ApplicationError> {
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {secret_key}")).map_err(|_| secret_unavailable())?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(
        "stripe-version",
        HeaderValue::from_static(STRIPE_API_VERSION),
    );
    Ok(headers)
}

async fn parse_stripe_response(
    response: reqwest::Response,
) -> Result<StripeObject, ApplicationError> {
    let status = response.status();
    if status.is_success() {
        return response
            .json::<StripeObject>()
            .await
            .map_err(|_| provider_invalid_response());
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(ApplicationError::Unavailable {
            service: "stripe",
            source: anyhow::anyhow!("Stripe returned HTTP {status}"),
        })
    } else {
        Err(ApplicationError::Conflict {
            code: "stripe_request_rejected",
            message: "Stripe rejected the payment operation",
        })
    }
}

async fn parse_stripe_account_response(
    response: reqwest::Response,
) -> Result<StripeAccount, ApplicationError> {
    let status = response.status();
    if status.is_success() {
        return response
            .json::<StripeAccount>()
            .await
            .map_err(|_| provider_invalid_response());
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(ApplicationError::Unavailable {
            service: "stripe",
            source: anyhow::anyhow!("Stripe returned HTTP {status}"),
        })
    } else {
        Err(ApplicationError::Conflict {
            code: "stripe_account_rejected",
            message: "Stripe rejected the connected account lookup",
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
        .chunks_exact(2)
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

fn is_platform_account(value: &str) -> bool {
    value
        .strip_prefix(STRIPE_PLATFORM_ACCOUNT_PREFIX)
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 255 - STRIPE_PLATFORM_ACCOUNT_PREFIX.len()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
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

fn provider_invalid_response() -> ApplicationError {
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

fn ignored_webhook() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "payload",
            reason: "contains an unsupported Stripe event type".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::State,
        http::{Request, Response},
        routing::any,
    };
    use chaos_application::ports::PaymentWebhookConfiguration;
    use chaos_domain::CurrencyCode;

    use super::*;

    const TEST_STRIPE_PLATFORM_ACCOUNT: &str = "platform:demo-store";

    struct StaticSecrets(HashMap<String, String>);

    #[async_trait]
    impl PaymentSecretResolver for StaticSecrets {
        async fn resolve(
            &self,
            reference: &PaymentSecretReference,
        ) -> Result<SecretString, ApplicationError> {
            self.0
                .get(reference.expose_reference())
                .cloned()
                .map(SecretString::from)
                .ok_or_else(secret_unavailable)
        }
    }

    struct StaticWebhookConfiguration(Vec<PaymentSecretReference>);

    #[async_trait]
    impl PaymentWebhookConfigurationRepository for StaticWebhookConfiguration {
        async fn webhook_configurations(
            &self,
            provider: &str,
            external_account_reference: Option<&str>,
        ) -> Result<Vec<PaymentWebhookConfiguration>, ApplicationError> {
            let reference = match (provider, external_account_reference) {
                ("stripe", Some("acct_connected")) => "acct_connected",
                ("stripe_checkout", None) => TEST_STRIPE_PLATFORM_ACCOUNT,
                _ => return Ok(Vec::new()),
            };
            Ok(self
                .0
                .iter()
                .cloned()
                .map(|secret_reference| PaymentWebhookConfiguration {
                    external_account_reference: reference.into(),
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
            ("GET", "/v1/accounts/acct_connected") => {
                r#"{"id":"acct_connected","charges_enabled":true,"payouts_enabled":true,"details_submitted":true,"capabilities":{"card_payments":"active"},"controller":{"fees":{"payer":"account"},"losses":{"payments":"stripe"}},"requirements":{"currently_due":[],"past_due":[],"disabled_reason":null}}"#
            }
            ("GET", "/v1/accounts/acct_not_ready") => {
                r#"{"id":"acct_not_ready","charges_enabled":false,"payouts_enabled":false,"details_submitted":false,"capabilities":{"card_payments":"inactive"},"controller":{"fees":{"payer":"application"},"losses":{"payments":"application"}},"requirements":{"currently_due":["business_profile.url"],"past_due":[],"disabled_reason":"requirements.past_due"}}"#
            }
            ("GET", "/v1/account") => {
                r#"{"id":"acct_platform","charges_enabled":true,"payouts_enabled":true,"details_submitted":true,"capabilities":{"card_payments":"active"},"requirements":{"currently_due":[],"past_due":[],"disabled_reason":null}}"#
            }
            ("POST", "/v1/refunds") => r#"{"id":"re_created"}"#,
            ("POST", "/v1/checkout/sessions") => {
                r#"{"id":"cs_created","client_secret":"cs_created_secret_value"}"#
            }
            ("GET", "/v1/checkout/sessions/cs_created") => {
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
    async fn stripe_adapter_executes_payment_client_handoff_and_refund_over_http() {
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
        let provider = StripePaymentProvider::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let aggregate_id = Uuid::now_v7();
        let created = provider
            .execute(ProviderCommand {
                event_type: "payment.create_requested".into(),
                aggregate_id,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "payment-command".into(),
                external_account_reference: "acct_connected".into(),
                credential_secret_reference: reference.clone(),
                payment_provider_reference: None,
                return_url: None,
            })
            .await
            .unwrap();
        assert_eq!(created.provider_reference, "pi_created");
        let action = provider
            .client_action(ProviderClientActionCommand {
                provider_reference: created.provider_reference.clone(),
                external_account_reference: "acct_connected".into(),
                credential_secret_reference: reference.clone(),
            })
            .await
            .unwrap();
        assert_eq!(action.public_key.expose_secret(), "pk_test_public");
        assert_eq!(
            action.client_token.expose_secret(),
            "pi_created_secret_value"
        );
        let refunded = provider
            .execute(ProviderCommand {
                event_type: "refund.create_requested".into(),
                aggregate_id: Uuid::now_v7(),
                amount_minor: 400,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "refund-command".into(),
                external_account_reference: "acct_connected".into(),
                credential_secret_reference: reference.clone(),
                payment_provider_reference: Some(created.provider_reference),
                return_url: None,
            })
            .await
            .unwrap();
        assert_eq!(refunded.provider_reference, "re_created");
        let checked_at = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let readiness = provider
            .check_readiness("acct_connected", &reference, checked_at)
            .await
            .unwrap();
        assert!(readiness.ready);
        assert!(readiness.blocker_codes.is_empty());
        assert_eq!(readiness.checked_at, checked_at);
        assert_eq!(readiness.configuration["fee_payer"], "account");
        let not_ready = provider
            .check_readiness("acct_not_ready", &reference, checked_at)
            .await
            .unwrap();
        assert!(!not_ready.ready);
        assert_eq!(
            not_ready.blocker_codes,
            vec![
                "charges_disabled",
                "payouts_disabled",
                "details_incomplete",
                "card_payments_inactive",
                "requirements_due",
                "fee_payer_mismatch",
                "loss_liability_mismatch"
            ]
        );

        let requests = state.0.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/v1/payment_intents");
        assert_eq!(requests[0].headers["stripe-account"], "acct_connected");
        assert_eq!(requests[0].headers["stripe-version"], STRIPE_API_VERSION);
        assert_eq!(requests[0].headers["idempotency-key"], "payment-command");
        assert_eq!(requests[0].headers[AUTHORIZATION], "Bearer sk_test_secret");
        let payment_form: HashMap<_, _> =
            url::form_urlencoded::parse(requests[0].body.as_bytes()).collect();
        assert_eq!(payment_form["amount"], "1234");
        assert_eq!(payment_form["currency"], "usd");
        assert_eq!(
            payment_form["metadata[chaos_payment_attempt_id]"],
            aggregate_id.to_string()
        );
        let refund_form: HashMap<_, _> =
            url::form_urlencoded::parse(requests[2].body.as_bytes()).collect();
        assert_eq!(refund_form["payment_intent"], "pi_created");
        assert_eq!(refund_form["amount"], "400");
        assert_eq!(requests[3].path, "/v1/accounts/acct_connected");
        assert!(requests[3].headers.get("stripe-account").is_none());
        assert_eq!(requests[3].headers["stripe-version"], STRIPE_API_VERSION);
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
        let provider = StripeCheckoutPaymentProvider::new(
            format!("http://{address}/").parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        assert_eq!(PaymentProvider::name(&provider), "stripe_checkout");
        let aggregate_id = Uuid::now_v7();
        let created = provider
            .execute(ProviderCommand {
                event_type: "payment.create_requested".into(),
                aggregate_id,
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "checkout-command".into(),
                external_account_reference: TEST_STRIPE_PLATFORM_ACCOUNT.into(),
                credential_secret_reference: reference.clone(),
                payment_provider_reference: None,
                return_url: Some("https://shop.example.com/success".into()),
            })
            .await
            .unwrap();
        assert_eq!(created.provider_reference, "cs_created");
        let action = provider
            .client_action(ProviderClientActionCommand {
                provider_reference: created.provider_reference.clone(),
                external_account_reference: TEST_STRIPE_PLATFORM_ACCOUNT.into(),
                credential_secret_reference: reference.clone(),
            })
            .await
            .unwrap();
        assert_eq!(action.kind, "mount_embedded_checkout");
        assert_eq!(
            action.client_token.expose_secret(),
            "cs_created_secret_value"
        );
        assert_eq!(action.account_reference, TEST_STRIPE_PLATFORM_ACCOUNT);
        let readiness = provider
            .check_readiness(
                TEST_STRIPE_PLATFORM_ACCOUNT,
                &reference,
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        assert!(readiness.ready);

        let requests = state.0.lock().unwrap();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/v1/checkout/sessions");
        assert!(requests[0].headers.get("stripe-account").is_none());
        assert!(requests[1].headers.get("stripe-account").is_none());
        assert_eq!(requests[2].path, "/v1/account");
        let form: HashMap<_, _> =
            url::form_urlencoded::parse(requests[0].body.as_bytes()).collect();
        assert_eq!(form["mode"], "payment");
        assert_eq!(form["ui_mode"], "embedded_page");
        assert_eq!(form["return_url"], "https://shop.example.com/success");
        assert_eq!(form["line_items[0][quantity]"], "1");
        assert_eq!(form["line_items[0][price_data][currency]"], "usd");
        assert_eq!(form["line_items[0][price_data][unit_amount]"], "1234");
        assert_eq!(
            form["metadata[chaos_payment_attempt_id]"],
            aggregate_id.to_string()
        );
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
        let provider = StripeCheckoutPaymentProvider::new(
            "http://127.0.0.1:1/".parse().unwrap(),
            Duration::from_secs(2),
            secrets,
        )
        .unwrap();
        let result = provider
            .execute(ProviderCommand {
                event_type: "payment.create_requested".into(),
                aggregate_id: Uuid::now_v7(),
                amount_minor: 1234,
                currency: CurrencyCode::parse("USD").unwrap(),
                idempotency_key: "checkout-command".into(),
                external_account_reference: "acct_connected".into(),
                credential_secret_reference: reference,
                payment_provider_reference: None,
                return_url: None,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stripe_checkout_webhook_supports_platform_account_events() {
        let active_reference =
            PaymentSecretReference::new("webhook", "test://webhook-active").unwrap();
        let previous_reference =
            PaymentSecretReference::new("webhook", "test://webhook-previous").unwrap();
        let verifier = StripeWebhookVerifier::for_provider(
            "stripe_checkout",
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
                "metadata": {"chaos_payment_attempt_id": aggregate_id}
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
                "stripe_checkout",
                &format!("t={},v1={signature}", received_at.unix_timestamp()),
                &payload,
                received_at,
            )
            .await
            .unwrap();
        assert_eq!(event.event_type, "payment.captured");
        assert_eq!(event.object_reference, "cs_created");
        assert_eq!(
            event.external_account_reference,
            TEST_STRIPE_PLATFORM_ACCOUNT
        );
        assert_eq!(event.payload["aggregate_id"], aggregate_id.to_string());

        assert!(
            verifier
                .verify(
                    "stripe_checkout",
                    &format!("t={},v1={signature}", received_at.unix_timestamp()),
                    &payload,
                    received_at + time::Duration::minutes(6),
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
            "metadata": {"chaos_payment_attempt_id": aggregate_id}
        });
        if let Some(status) = payment_status {
            object["payment_status"] = serde_json::Value::String(status.into());
        }
        serde_json::from_value(serde_json::json!({
            "id": "evt_1",
            "type": event_type,
            "account": "acct_connected",
            "data": {"object": object}
        }))
        .unwrap()
    }

    #[test]
    fn checkout_session_completed_paid_captures_immediately() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.completed", Some("paid"), aggregate_id);
        let (event_type, resolved_id, failure_code) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, "payment.captured");
        assert_eq!(resolved_id, aggregate_id);
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
        assert_eq!(event_type, "payment.captured");
    }

    #[test]
    fn checkout_session_completed_unpaid_is_ignored_pending_the_async_follow_up() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.completed", Some("unpaid"), aggregate_id);
        assert!(map_stripe_event(&event).is_err());
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
        assert_eq!(event_type, "payment.captured");
    }

    #[test]
    fn checkout_session_async_payment_failed_fails() {
        let aggregate_id = Uuid::now_v7();
        let event =
            checkout_session_event("checkout.session.async_payment_failed", None, aggregate_id);
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, "payment.failed");
    }

    #[test]
    fn checkout_session_expired_cancels() {
        let aggregate_id = Uuid::now_v7();
        let event = checkout_session_event("checkout.session.expired", None, aggregate_id);
        let (event_type, ..) = map_stripe_event(&event).unwrap();
        assert_eq!(event_type, "payment.cancelled");
    }
}
