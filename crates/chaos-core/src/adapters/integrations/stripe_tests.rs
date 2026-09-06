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
    assert_eq!(action.kind, "stripe_checkout_embedded");
    assert_eq!(
        action.client_token.expose_secret(),
        "cs_created_secret_value"
    );
    let requests = state.0.lock().unwrap();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/v1/checkout/sessions");
    assert!(requests[0].headers.get("stripe-account").is_none());
    let form: HashMap<_, _> = url::form_urlencoded::parse(requests[0].body.as_bytes()).collect();
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
    let active_reference = PaymentSecretReference::new("webhook", "test://webhook-active").unwrap();
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
    let event = checkout_session_event("checkout.session.completed", Some("paid"), aggregate_id);
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
    let event = checkout_session_event("checkout.session.completed", Some("unpaid"), aggregate_id);
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
    let event = checkout_session_event("checkout.session.async_payment_failed", None, aggregate_id);
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
