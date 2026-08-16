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

pub struct StripePaymentProvider {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn PaymentSecretResolver>,
}

impl StripePaymentProvider {
    pub fn new(
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

    async fn retrieve_payment_intent(
        &self,
        credentials: &StripeCredentials,
        connected_account: &str,
        provider_reference: &str,
    ) -> Result<StripeObject, ApplicationError> {
        if !valid_stripe_identifier(provider_reference, "pi_") {
            return Err(provider_invalid_response());
        }
        let response = self
            .client
            .get(self.endpoint(&format!("v1/payment_intents/{provider_reference}"))?)
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
            .credentials(&command.credential_secret_reference)
            .await?;
        let (object, expected_prefix) = if command.event_type == "payment.create_requested" {
            self.send_form(
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
            self.send_form(
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
        if !valid_stripe_identifier(external_account_reference, "acct_") {
            return Err(provider_invalid_response());
        }
        let credentials = self.credentials(credential_secret_reference).await?;
        let response = self
            .client
            .get(self.endpoint(&format!("v1/accounts/{external_account_reference}"))?)
            .headers(stripe_platform_headers(
                credentials.secret_key.expose_secret(),
            )?)
            .send()
            .await
            .map_err(provider_network_error)?;
        let account = parse_stripe_account_response(response).await?;
        if account.id != external_account_reference {
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
        if fee_payer != Some("account") {
            blocker_codes.push("fee_payer_mismatch".into());
        }
        if losses_payer != Some("stripe") {
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
}

pub struct StripeWebhookVerifier {
    configurations: Arc<dyn PaymentWebhookConfigurationRepository>,
    secrets: Arc<dyn PaymentSecretResolver>,
}

impl StripeWebhookVerifier {
    pub fn new(
        configurations: Arc<dyn PaymentWebhookConfigurationRepository>,
        secrets: Arc<dyn PaymentSecretResolver>,
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
        if !valid_stripe_identifier(&envelope.id, "evt_")
            || !valid_stripe_identifier(&envelope.account, "acct_")
        {
            return Err(invalid_webhook());
        }
        let references = self
            .configurations
            .webhook_secret_references(provider, &envelope.account)
            .await?;
        if references.is_empty() {
            return Err(ApplicationError::Unauthorized);
        }
        let mut verified = false;
        for reference in references {
            let secret = self.secrets.resolve(&reference).await?;
            if verify_stripe_signature(signature, payload, secret.expose_secret(), received_at)
                .is_ok()
            {
                verified = true;
                break;
            }
        }
        if !verified {
            return Err(ApplicationError::Unauthorized);
        }
        let (event_type, aggregate_id, failure_code) = map_stripe_event(&envelope)?;
        let object_reference = envelope.data.object.id.clone();
        Ok(VerifiedWebhookEvent {
            provider: provider.into(),
            provider_event_id: envelope.id,
            event_type,
            external_account_reference: envelope.account,
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
    account: String,
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
    if !valid_stripe_identifier(connected_account, "acct_") {
        return Err(provider_invalid_response());
    }
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {secret_key}")).map_err(|_| secret_unavailable())?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(
        "stripe-account",
        HeaderValue::from_str(connected_account).map_err(|_| provider_invalid_response())?,
    );
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
    use chaos_domain::CurrencyCode;

    use super::*;

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
        async fn webhook_secret_references(
            &self,
            provider: &str,
            external_account_reference: &str,
        ) -> Result<Vec<PaymentSecretReference>, ApplicationError> {
            Ok(
                if provider == "stripe" && external_account_reference == "acct_connected" {
                    self.0.clone()
                } else {
                    Vec::new()
                },
            )
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
            ("POST", "/v1/refunds") => r#"{"id":"re_created"}"#,
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
    async fn stripe_webhook_verifies_timestamped_signature_before_mapping_event() {
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
            "type": "payment_intent.succeeded",
            "account": "acct_connected",
            "data": {"object": {
                "id": "pi_created",
                "status": "succeeded",
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
                "stripe",
                &format!("t={},v1={signature}", received_at.unix_timestamp()),
                &payload,
                received_at,
            )
            .await
            .unwrap();
        assert_eq!(event.event_type, "payment.captured");
        assert_eq!(event.object_reference, "pi_created");
        assert_eq!(event.payload["aggregate_id"], aggregate_id.to_string());

        assert!(
            verifier
                .verify(
                    "stripe",
                    &format!("t={},v1={signature}", received_at.unix_timestamp()),
                    &payload,
                    received_at + time::Duration::minutes(6),
                )
                .await
                .is_err()
        );
    }
}
