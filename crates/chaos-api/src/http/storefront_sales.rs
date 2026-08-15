use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, CheckoutDetail, CheckoutLineItem, IdempotencyRequest,
        OrderDetail, OrderLineItem,
    },
    sales::{
        CheckoutContactInput, CreateCartInput, CreateCheckoutInput, CreateOrderInput,
        PostalAddressInput, QuoteShippingInput, RemoveCartLineInput, SetCartLineInput,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{ShippingSelection, ShippingServiceId},
    sales::{CartId, CheckoutId, OrderId, ShopperId},
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CartMachine, CartShopper,
    CheckoutShopper, merchant::idempotency_key,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shopper-sessions", post(create_shopper_session))
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route(
            "/carts/{cart_id}/lines/{product_variant_id}",
            axum::routing::put(set_cart_line).delete(remove_cart_line),
        )
        .route("/carts/{cart_id}/checkout", post(create_checkout))
        .route("/carts/{cart_id}/shipping-options", post(quote_shipping))
        .route("/checkouts/{checkout_id}", get(get_checkout))
        .route("/checkouts/{checkout_id}/order", post(create_order))
        .route("/orders/{order_id}", get(get_order))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCartBody {
    currency: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetCartLineBody {
    quantity: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckoutContactBody {
    email: String,
    phone: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PostalAddressBody {
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCheckoutBody {
    contact: CheckoutContactBody,
    billing_address: PostalAddressBody,
    shipping_address: Option<PostalAddressBody>,
    shipping_service_id: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QuoteShippingBody {
    destination_country: String,
}

#[derive(Deserialize)]
struct CartPath {
    cart_id: Uuid,
}

#[derive(Deserialize)]
struct CartLinePath {
    cart_id: Uuid,
    product_variant_id: Uuid,
}

#[derive(Deserialize)]
struct CheckoutPath {
    checkout_id: Uuid,
}

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
}

#[derive(Serialize)]
struct CartLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize)]
struct CartData {
    id: Uuid,
    price_list_id: Uuid,
    currency: String,
    status: &'static str,
    version: u64,
    lines: Vec<CartLineData>,
    subtotal_amount_minor: i64,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    shopper_token: Option<String>,
}

#[derive(Serialize)]
struct ShopperSessionData {
    shopper_token: String,
}

#[derive(Serialize)]
struct CheckoutLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize)]
struct CheckoutData {
    id: Uuid,
    cart_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    status: String,
    contact: CheckoutContactData,
    billing_address: PostalAddressData,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<ShippingSelectionData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    expires_at: ApiDateTime,
    lines: Vec<CheckoutLineData>,
    created_at: ApiDateTime,
}

#[derive(Serialize)]
struct OrderLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize)]
struct OrderTransitionData {
    id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_status: Option<&'static str>,
    to_status: &'static str,
    kind: String,
    occurred_at: ApiDateTime,
}

#[derive(Serialize)]
pub(super) struct OrderData {
    id: Uuid,
    checkout_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    status: &'static str,
    contact: CheckoutContactData,
    billing_address: PostalAddressData,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<ShippingSelectionData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineData>,
    transitions: Vec<OrderTransitionData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct CheckoutContactData {
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
}

#[derive(Serialize)]
struct ShippingSelectionData {
    service_id: Uuid,
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
}

#[derive(Serialize)]
struct PostalAddressData {
    full_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    company: Option<String>,
    address_line1: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    address_line2: Option<String>,
    locality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    administrative_area: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    postal_code: Option<String>,
    country_code: String,
}

async fn create_shopper_session(
    State(state): State<ApiState>,
    CartMachine(actor): CartMachine,
) -> Result<ApiResponse<ShopperSessionData>, ApiError> {
    let shopper_token = state.shopper_credentials.issue(&actor, ShopperId::new())?;
    Ok(ApiResponse::created(ShopperSessionData {
        shopper_token: shopper_token.expose_secret().to_owned(),
    }))
}

async fn create_cart(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiJson(body): ApiJson<CreateCartBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let idempotency = body_request(&headers, "create_cart", &body)?;
    let machine = actor.machine.clone();
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput {
            actor,
            currency: body.currency,
            idempotency,
        })
        .await?;
    let shopper_token = state.shopper_credentials.issue(&machine, cart.shopper_id)?;
    Ok(ApiResponse::created(cart_data(
        cart,
        Some(shopper_token.expose_secret().to_owned()),
    )?))
}

async fn get_cart(
    State(state): State<ApiState>,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let cart = state
        .storefront_sales
        .get_cart(&actor, CartId::from_uuid(path.cart_id))
        .await?;
    Ok(ApiResponse::ok(cart_data(cart, None)?))
}

async fn set_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartLinePath>,
    ApiJson(body): ApiJson<SetCartLineBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let idempotency = body_request(
        &headers,
        "set_cart_line",
        &(path.cart_id, path.product_variant_id, &body),
    )?;
    let cart = state
        .storefront_sales
        .set_cart_line(SetCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            quantity: body.quantity,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(cart_data(cart, None)?))
}

async fn remove_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartLinePath>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let idempotency = body_request(
        &headers,
        "remove_cart_line",
        &(path.cart_id, path.product_variant_id),
    )?;
    let cart = state
        .storefront_sales
        .remove_cart_line(RemoveCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(cart_data(cart, None)?))
}

async fn create_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(body): ApiJson<CreateCheckoutBody>,
) -> Result<ApiResponse<CheckoutData>, ApiError> {
    let idempotency = body_request(&headers, "create_checkout", &(path.cart_id, &body))?;
    let checkout = state
        .storefront_sales
        .create_checkout(CreateCheckoutInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            contact: contact_input(body.contact),
            billing_address: address_input(body.billing_address),
            shipping_address: body.shipping_address.map(address_input),
            shipping_service_id: body.shipping_service_id.map(ShippingServiceId::from_uuid),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(checkout_data(checkout)?))
}

async fn quote_shipping(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(body): ApiJson<QuoteShippingBody>,
) -> Result<ApiResponse<Vec<ShippingSelectionData>>, ApiError> {
    let quotes = state
        .storefront_sales
        .quote_shipping(QuoteShippingInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            destination_country: body.destination_country,
        })
        .await?;
    Ok(ApiResponse::ok(quotes.iter().map(shipping_data).collect()))
}

async fn get_checkout(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CheckoutPath>,
) -> Result<ApiResponse<CheckoutData>, ApiError> {
    let checkout = state
        .storefront_sales
        .get_checkout(&actor, CheckoutId::from_uuid(path.checkout_id))
        .await?;
    Ok(ApiResponse::ok(checkout_data(checkout)?))
}

async fn create_order(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CheckoutPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let idempotency = body_request(&headers, "create_order", &path.checkout_id)?;
    let order = state
        .storefront_sales
        .create_order(CreateOrderInput {
            actor,
            checkout_id: CheckoutId::from_uuid(path.checkout_id),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(order_data(order)?))
}

async fn get_order(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let order = state
        .storefront_sales
        .get_order(&actor, OrderId::from_uuid(path.order_id))
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

fn contact_input(value: CheckoutContactBody) -> CheckoutContactInput {
    CheckoutContactInput {
        email: value.email,
        phone: value.phone,
    }
}

fn address_input(value: PostalAddressBody) -> PostalAddressInput {
    PostalAddressInput {
        full_name: value.full_name,
        company: value.company,
        address_line1: value.address_line1,
        address_line2: value.address_line2,
        locality: value.locality,
        administrative_area: value.administrative_area,
        postal_code: value.postal_code,
        country_code: value.country_code,
    }
}

fn contact_data(value: &chaos_domain::sales::CheckoutContact) -> CheckoutContactData {
    CheckoutContactData {
        email: value.email().into(),
        phone: value.phone().map(str::to_owned),
    }
}

fn address_data(value: &chaos_domain::sales::PostalAddress) -> PostalAddressData {
    PostalAddressData {
        full_name: value.full_name().into(),
        company: value.company().map(str::to_owned),
        address_line1: value.address_line1().into(),
        address_line2: value.address_line2().map(str::to_owned),
        locality: value.locality().into(),
        administrative_area: value.administrative_area().map(str::to_owned),
        postal_code: value.postal_code().map(str::to_owned),
        country_code: value.country_code().into(),
    }
}

fn shipping_data(value: &ShippingSelection) -> ShippingSelectionData {
    ShippingSelectionData {
        service_id: value.service_id().as_uuid(),
        code: value.code().into(),
        name: value.name().into(),
        amount_minor: value.amount().amount_minor(),
        currency: value.amount().currency().as_str().into(),
        estimated_min_days: value.estimated_min_days(),
        estimated_max_days: value.estimated_max_days(),
    }
}

fn body_request<T: Serialize>(
    headers: &HeaderMap,
    operation: &'static str,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(operation, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn cart_data(
    cart: CartDetail,
    shopper_token: Option<String>,
) -> Result<CartData, ApplicationError> {
    Ok(CartData {
        id: cart.id.as_uuid(),
        price_list_id: cart.price_list_id.as_uuid(),
        currency: cart.currency.as_str().to_owned(),
        status: cart.status.as_str(),
        version: cart.version,
        lines: cart.lines.into_iter().map(cart_line_data).collect(),
        subtotal_amount_minor: cart.subtotal_amount_minor,
        created_at: cart.created_at.into(),
        updated_at: cart.updated_at.into(),
        shopper_token,
    })
}

fn cart_line_data(line: CartLineItem) -> CartLineData {
    CartLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        requires_shipping: line.requires_shipping,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        tax_inclusive: line.tax_inclusive,
    }
}

fn checkout_data(checkout: CheckoutDetail) -> Result<CheckoutData, ApplicationError> {
    Ok(CheckoutData {
        id: checkout.id.as_uuid(),
        cart_id: checkout.cart_id.as_uuid(),
        inventory_reservation_id: checkout.inventory_reservation_id.map(|id| id.as_uuid()),
        price_list_id: checkout.price_list_id.as_uuid(),
        currency: checkout.currency.as_str().to_owned(),
        status: checkout.status,
        contact: contact_data(checkout.identity.contact()),
        billing_address: address_data(checkout.identity.billing_address()),
        shipping_address: checkout.identity.shipping_address().map(address_data),
        shipping: checkout.shipping.as_ref().map(shipping_data),
        subtotal_amount_minor: checkout.subtotal_amount_minor,
        discount_amount_minor: checkout.discount_amount_minor,
        tax_amount_minor: checkout.tax_amount_minor,
        shipping_amount_minor: checkout.shipping_amount_minor,
        total_amount_minor: checkout.total_amount_minor,
        expires_at: checkout.expires_at.into(),
        lines: checkout.lines.into_iter().map(checkout_line_data).collect(),
        created_at: checkout.created_at.into(),
    })
}

fn checkout_line_data(line: CheckoutLineItem) -> CheckoutLineData {
    CheckoutLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        requires_shipping: line.requires_shipping,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        discount_amount_minor: line.discount_amount_minor,
        tax_amount_minor: line.tax_amount_minor,
        total_amount_minor: line.total_amount_minor,
        tax_inclusive: line.tax_inclusive,
    }
}

pub(super) fn order_data(order: OrderDetail) -> Result<OrderData, ApplicationError> {
    Ok(OrderData {
        id: order.id.as_uuid(),
        checkout_id: order.checkout_id.as_uuid(),
        inventory_reservation_id: order.inventory_reservation_id.map(|id| id.as_uuid()),
        price_list_id: order.price_list_id.as_uuid(),
        currency: order.currency.as_str().to_owned(),
        status: order.status.as_str(),
        contact: contact_data(order.identity.contact()),
        billing_address: address_data(order.identity.billing_address()),
        shipping_address: order.identity.shipping_address().map(address_data),
        shipping: order.shipping.as_ref().map(shipping_data),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        shipping_amount_minor: order.shipping_amount_minor,
        total_amount_minor: order.total_amount_minor,
        lines: order.lines.into_iter().map(order_line_data).collect(),
        transitions: order
            .transitions
            .into_iter()
            .map(|transition| {
                Ok(OrderTransitionData {
                    id: transition.id,
                    from_status: transition.from_status.map(|status| status.as_str()),
                    to_status: transition.to_status.as_str(),
                    kind: transition.kind,
                    occurred_at: transition.occurred_at.into(),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: order.created_at.into(),
        updated_at: order.updated_at.into(),
    })
}

fn order_line_data(line: OrderLineItem) -> OrderLineData {
    OrderLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        requires_shipping: line.requires_shipping,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        discount_amount_minor: line.discount_amount_minor,
        tax_amount_minor: line.tax_amount_minor,
        total_amount_minor: line.total_amount_minor,
        tax_inclusive: line.tax_inclusive,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{HeaderValue, Method, Request, StatusCode},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chaos_application::ports::{ApiKeyMaterialGenerator, GeneratedApiKeyMaterial};
    use chaos_application::{payments::PaymentWorkers, ports::IntegrationQueue};
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        fulfillment::ShippingServiceId,
        identity::UserId,
        inventory::InventoryLocationId,
        merchant::{ApiKeyClass, ApiKeyId, ApiKeyMode, MerchantAccountId, SalesChannelId, StoreId},
        pricing::PriceListId,
    };
    use chaos_infrastructure::repositories::PostgresPaymentRepository;
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;
    use hmac::{Hmac, Mac};
    use secrecy::ExposeSecret;
    use serde_json::{Value, json};
    use sha2::Sha256;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    async fn insert_key(
        pool: &PgPool,
        account_id: MerchantAccountId,
        store_id: StoreId,
        user_id: UserId,
        scopes: &[&str],
    ) -> GeneratedApiKeyMaterial {
        let material =
            SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Publishable, ApiKeyMode::Live);
        let key_id = ApiKeyId::new();
        sqlx::query(
            "INSERT INTO merchant.api_keys \
             (id, merchant_account_id, store_id, key_identifier, secret_digest, \
              display_suffix, name, class, mode, created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, $6, 'Sales HTTP', 'publishable', 'live', $7)",
        )
        .bind(key_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(user_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
        for scope in scopes {
            sqlx::query(
                "INSERT INTO merchant.api_key_scopes (merchant_account_id, api_key_id, scope) \
                 VALUES ($1, $2, $3::merchant.api_key_scope)",
            )
            .bind(account_id.as_uuid())
            .bind(key_id.as_uuid())
            .bind(scope)
            .execute(pool)
            .await
            .unwrap();
        }
        material
    }

    fn store_request(
        method: Method,
        uri: &str,
        secret: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(secret) = secret {
            builder = builder.header("authorization", format!("Bearer {secret}"));
        }
        if let Some(key) = idempotency_key {
            builder = builder.header("idempotency-key", key);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap()
    }

    fn webhook_request(event: Value) -> Request<Body> {
        let body = event.to_string();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(b"test-payment-webhook-secret-32-bytes").unwrap();
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        Request::post("/webhooks/v1/payments/testpay")
            .header("content-type", "application/json")
            .header("x-payment-signature", signature)
            .body(Body::from(body))
            .unwrap()
    }

    fn with_shopper_token(mut request: Request<Body>, shopper_token: &str) -> Request<Body> {
        request.headers_mut().insert(
            "x-chaos-shopper-token",
            HeaderValue::from_str(shopper_token).unwrap(),
        );
        request
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn storefront_cart_and_checkout_http_matrix_is_scoped_idempotent_and_inventory_safe() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let suffix = Uuid::now_v7().simple().to_string();
        let provider_account_reference = format!("acct_{suffix}");
        let user_id = UserId::new();
        let account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let other_channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = PriceListId::new();
        let location_id = InventoryLocationId::new();
        let shipping_service_id = ShippingServiceId::new();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("sales-http-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Sales HTTP')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("sales-http-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, merchant_account_id, code, name, status) \
             VALUES ($1, $2, 'other-sales-http', 'Other Sales HTTP', 'active')",
        )
        .bind(other_store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.merchant_account_memberships \
             (merchant_account_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(account_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.store_currencies (merchant_account_id, store_id, currency) \
             VALUES ($1, $2, 'USD')",
        )
        .bind(account_id.as_uuid())
        .bind(other_store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, merchant_account_id, code, name, status) \
             VALUES ($1, $2, 'sales-http', 'Sales HTTP', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Web', 'web', true)",
        )
        .bind(other_channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(other_store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.store_currencies (merchant_account_id, store_id, currency) \
             VALUES ($1, $2, 'USD')",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fulfillment.shipping_services \
             (id, merchant_account_id, store_id, code, name, amount_minor, currency, \
              estimated_min_days, estimated_max_days) \
             VALUES ($1, $2, $3, 'standard', 'Standard shipping', 500, 'USD', 2, 5)",
        )
        .bind(shipping_service_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fulfillment.shipping_service_regions \
             (merchant_account_id, store_id, shipping_service_id, country_code) \
             VALUES ($1, $2, $3, 'US')",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(shipping_service_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Web', 'web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO payments.provider_accounts \
             (id, merchant_account_id, store_id, provider, external_account_reference) \
             VALUES ($1, $2, $3, 'testpay', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&provider_account_reference)
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, merchant_account_id, store_id, handle, title, status) \
             VALUES ($1, $2, $3, 'sales-product', 'Sales Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, merchant_account_id, store_id, product_id, title, status, track_inventory) \
             VALUES ($1, $2, $3, $4, 'Default', 'active', true)",
        )
        .bind(variant_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_publications \
             (merchant_account_id, store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(channel_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.price_lists \
             (id, merchant_account_id, store_id, code, name, currency, status) \
             VALUES ($1, $2, $3, 'usd', 'USD', 'USD', 'active')",
        )
        .bind(price_list_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, merchant_account_id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, $5, 1250)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.inventory_locations \
             (id, merchant_account_id, store_id, code, name) \
             VALUES ($1, $2, $3, 'primary', 'Primary')",
        )
        .bind(location_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.stock_items \
             (id, merchant_account_id, store_id, inventory_location_id, product_variant_id, \
              on_hand_quantity) VALUES ($1, $2, $3, $4, $5, 2)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(location_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let full_key = insert_key(
            &owner_pool,
            account_id,
            store_id,
            user_id,
            &["carts:write", "checkout:write"],
        )
        .await;
        let catalog_key = insert_key(
            &owner_pool,
            account_id,
            store_id,
            user_id,
            &["catalog:read"],
        )
        .await;
        let other_store_key = insert_key(
            &owner_pool,
            account_id,
            other_store_id,
            user_id,
            &["checkout:write"],
        )
        .await;
        let full_secret = full_key.plaintext.expose_secret();
        let catalog_secret = catalog_key.plaintext.expose_secret();
        let other_store_secret = other_store_key.plaintext.expose_secret();
        let state = test_state(&database_url, user_id);
        let test_clock = state.clock.clone();
        let runtime_pool = state.infrastructure.runtime_pool();
        let payment_repository = Arc::new(PostgresPaymentRepository::new(runtime_pool.clone()));
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            Vec::new(),
        );
        let app = router(state);

        let unauthorized = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/carts",
                None,
                Some("u"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let forbidden = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/carts",
                Some(catalog_secret),
                Some("f"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let shopper_session = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/shopper-sessions",
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(shopper_session.status(), StatusCode::CREATED);
        let shopper_session = response_json(shopper_session).await;
        let shopper_token = shopper_session["data"]["shopper_token"]
            .as_str()
            .unwrap()
            .to_owned();
        let missing_idempotency = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    "/store/v1/carts",
                    Some(full_secret),
                    None,
                    Some(json!({})),
                ),
                &shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(
            missing_idempotency.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let missing = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/carts/{}", Uuid::now_v7()),
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let create = with_shopper_token(
            store_request(
                Method::POST,
                "/store/v1/carts",
                Some(full_secret),
                Some("create"),
                Some(json!({})),
            ),
            &shopper_token,
        );
        let created = app.clone().oneshot(create).await.unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let cart_id = created["data"]["id"].as_str().unwrap();
        assert_eq!(created["data"]["shopper_token"], shopper_token);
        let shopper_token = shopper_token.as_str();
        let unrelated_session = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/shopper-sessions",
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        let unrelated_session = response_json(unrelated_session).await;
        let unrelated_token = unrelated_session["data"]["shopper_token"].as_str().unwrap();
        let cross_shopper = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/carts/{cart_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                unrelated_token,
            ))
            .await
            .unwrap();
        assert_eq!(cross_shopper.status(), StatusCode::NOT_FOUND);

        let line_uri = format!("/store/v1/carts/{cart_id}/lines/{}", variant_id.as_uuid());
        let invalid_line = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::PUT,
                    &line_uri,
                    Some(full_secret),
                    Some("invalid-line"),
                    Some(json!({"quantity": 0})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_line.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let updated = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::PUT,
                    &line_uri,
                    Some(full_secret),
                    Some("line"),
                    Some(json!({"quantity": 2})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = response_json(updated).await;
        assert_eq!(updated["data"]["subtotal_amount_minor"], 2500);

        let quotes = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &format!("/store/v1/carts/{cart_id}/shipping-options"),
                    Some(full_secret),
                    None,
                    Some(json!({"destination_country": "us"})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(quotes.status(), StatusCode::OK);
        let quotes = response_json(quotes).await;
        assert_eq!(
            quotes["data"][0]["service_id"],
            shipping_service_id.as_uuid().to_string()
        );
        assert_eq!(quotes["data"][0]["amount_minor"], 500);

        let checkout_uri = format!("/store/v1/carts/{cart_id}/checkout");
        let address = json!({
            "full_name": "Guest Buyer",
            "address_line1": "1 Market Street",
            "locality": "San Francisco",
            "administrative_area": "CA",
            "postal_code": "94105",
            "country_code": "US"
        });
        let checkout_body = json!({
            "contact": {
                "email": " Guest@Example.COM ",
                "phone": "+14155552671"
            },
            "billing_address": address.clone(),
            "shipping_address": address.clone(),
            "shipping_service_id": shipping_service_id.as_uuid()
        });
        sqlx::query("UPDATE fulfillment.shipping_services SET status = 'archived' WHERE id = $1")
            .bind(shipping_service_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        let stale_shipping = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-stale-shipping"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(stale_shipping.status(), StatusCode::UNPROCESSABLE_ENTITY);
        sqlx::query("UPDATE fulfillment.shipping_services SET status = 'active' WHERE id = $1")
            .bind(shipping_service_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        let invalid_contact = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-invalid-contact"),
                    Some(json!({
                        "contact": {"email": "invalid"},
                        "billing_address": address.clone(),
                        "shipping_address": address.clone()
                    })),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_contact.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let missing_shipping = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-missing-shipping"),
                    Some(json!({
                        "contact": {"email": "guest@example.com"},
                        "billing_address": address.clone()
                    })),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(missing_shipping.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let checkout = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(checkout.status(), StatusCode::CREATED);
        let checkout = response_json(checkout).await;
        let checkout_id = checkout["data"]["id"].as_str().unwrap();
        assert_eq!(checkout["data"]["shipping_amount_minor"], 500);
        assert_eq!(checkout["data"]["total_amount_minor"], 3000);
        assert_eq!(
            checkout["data"]["lines"][0]["product_title"],
            "Sales Product"
        );
        assert!(checkout["data"]["inventory_reservation_id"].is_string());
        assert_eq!(checkout["data"]["contact"]["email"], "guest@example.com");
        assert_eq!(checkout["data"]["shipping_address"]["country_code"], "US");

        let replay = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(replay).await["data"]["id"], checkout_id);

        let order_uri = format!("/store/v1/checkouts/{checkout_id}/order");
        let order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &order_uri,
                    Some(full_secret),
                    Some("order"),
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(order.status(), StatusCode::CREATED);
        let order = response_json(order).await;
        let order_id = order["data"]["id"].as_str().unwrap();
        assert_eq!(order["data"]["status"], "pending");
        assert_eq!(order["data"]["shipping_amount_minor"], 500);
        assert_eq!(order["data"]["total_amount_minor"], 3000);
        assert_eq!(order["data"]["contact"]["email"], "guest@example.com");
        assert_eq!(
            order["data"]["billing_address"]["address_line1"],
            "1 Market Street"
        );
        assert_eq!(order["data"]["transitions"][0]["kind"], "created");
        let order_replay = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &order_uri,
                    Some(full_secret),
                    Some("order"),
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(order_replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(order_replay).await["data"]["id"], order_id);
        let unrelated_order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/orders/{order_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                unrelated_token,
            ))
            .await
            .unwrap();
        assert_eq!(unrelated_order.status(), StatusCode::NOT_FOUND);

        let mut snapshot_connection = runtime_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, false)")
            .bind(account_id.as_uuid().to_string())
            .execute(&mut *snapshot_connection)
            .await
            .unwrap();
        assert!(
            sqlx::query(
                "UPDATE sales.order_contacts SET email = 'tampered@example.com' \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_addresses SET address_line1 = 'Tampered' \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.checkout_shipping_selections SET amount_minor = 1 \
                 WHERE checkout_id = $1",
            )
            .bind(Uuid::parse_str(checkout_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_shipping_selections SET amount_minor = 1 \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM sales.orders WHERE id = $1")
                .bind(Uuid::parse_str(order_id).unwrap())
                .execute(&mut *snapshot_connection)
                .await
                .is_err()
        );

        let terminal = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-again"),
                    Some(checkout_body),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(terminal.status(), StatusCode::CONFLICT);

        let fetched = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/checkouts/{checkout_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_eq!(response_json(fetched).await["data"]["id"], checkout_id);

        let fetched_order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/orders/{order_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(fetched_order.status(), StatusCode::OK);
        assert_eq!(response_json(fetched_order).await["data"]["id"], order_id);

        let payment_attempt = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &format!("/store/v1/orders/{order_id}/payment-attempts"),
                    Some(full_secret),
                    Some("payment-attempt"),
                    Some(json!({"provider": "testpay"})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(payment_attempt.status(), StatusCode::CREATED);
        let payment_attempt = response_json(payment_attempt).await;
        let payment_attempt_id = payment_attempt["data"]["id"].as_str().unwrap();
        assert_eq!(payment_attempt["data"]["status"], "pending");
        let cross_store = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                    Some(other_store_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(cross_store.status(), StatusCode::UNAUTHORIZED);

        let invalid_webhook = app
            .clone()
            .oneshot(
                Request::post("/webhooks/v1/payments/testpay")
                    .header("content-type", "application/json")
                    .header("x-payment-signature", "invalid")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_webhook.status(), StatusCode::UNAUTHORIZED);

        for (event_id, event_type) in [
            ("evt_authorized", "payment.authorized"),
            ("evt_captured", "payment.captured"),
        ] {
            let event = json!({
                "id": format!("{event_id}-{suffix}"),
                "event_type": event_type,
                "account": &provider_account_reference,
                "object": "pay_provider_1",
                "aggregate_id": payment_attempt_id,
            });
            let webhook = app
                .clone()
                .oneshot(webhook_request(event.clone()))
                .await
                .unwrap();
            assert_eq!(webhook.status(), StatusCode::ACCEPTED);
            assert_eq!(response_json(webhook).await["data"]["accepted"], true);
            let duplicate = app.clone().oneshot(webhook_request(event)).await.unwrap();
            assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
            assert_eq!(response_json(duplicate).await["data"]["accepted"], false);
            assert_eq!(
                payment_workers
                    .run_webhook_batch(Uuid::now_v7(), test_clock.now(), 10)
                    .await
                    .unwrap(),
                1
            );
        }

        let captured = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(captured.status(), StatusCode::OK);
        assert_eq!(response_json(captured).await["data"]["status"], "captured");

        let admin_order_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/orders/{order_id}",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let invalid_cancel = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{admin_order_uri}/cancel"),
                Some("cancel-confirmed"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_cancel.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let refund = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!(
                    "/admin/v1/merchant-accounts/{}/stores/{}/payment-attempts/{payment_attempt_id}/refunds",
                    account_id.as_uuid(),
                    store_id.as_uuid()
                ),
                Some("refund"),
                Some(json!({"amount_minor": 1000})),
            ))
            .await
            .unwrap();
        assert_eq!(refund.status(), StatusCode::CREATED);
        let refund = response_json(refund).await;
        let refund_id = refund["data"]["id"].as_str().unwrap();
        let refund_webhook = app
            .clone()
            .oneshot(webhook_request(json!({
                "id": format!("evt_refund-{suffix}"),
                "event_type": "refund.succeeded",
                "account": &provider_account_reference,
                "object": "refund_provider_1",
                "aggregate_id": refund_id,
            })))
            .await
            .unwrap();
        assert_eq!(refund_webhook.status(), StatusCode::ACCEPTED);
        assert_eq!(
            payment_workers
                .run_webhook_batch(Uuid::now_v7(), test_clock.now(), 10)
                .await
                .unwrap(),
            1
        );
        let refund_status: String =
            sqlx::query_scalar("SELECT status::text FROM payments.refunds WHERE id = $1")
                .bind(Uuid::parse_str(refund_id).unwrap())
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(refund_status, "succeeded");

        let first_worker = Uuid::now_v7();
        let second_worker = Uuid::now_v7();
        let claimed_at = test_clock.now();
        let (first_jobs, second_jobs) = tokio::join!(
            payment_repository.claim_outbox(
                first_worker,
                10,
                claimed_at,
                claimed_at - time::Duration::minutes(1),
            ),
            payment_repository.claim_outbox(
                second_worker,
                10,
                claimed_at,
                claimed_at - time::Duration::minutes(1),
            ),
        );
        let first_jobs = first_jobs.unwrap();
        let second_jobs = second_jobs.unwrap();
        assert_eq!(first_jobs.len() + second_jobs.len(), 2);
        let first_ids = first_jobs.iter().map(|job| job.id).collect::<Vec<_>>();
        assert!(second_jobs.iter().all(|job| !first_ids.contains(&job.id)));
        for job in first_jobs {
            payment_repository
                .finish_outbox(first_worker, job.id, Ok(()), claimed_at)
                .await
                .unwrap();
        }
        for job in second_jobs {
            payment_repository
                .finish_outbox(second_worker, job.id, Ok(()), claimed_at)
                .await
                .unwrap();
        }

        let recoverable_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.outbox_events \
             (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, $3, 'payment_attempt', $4, 'payment.create_requested', '{}'::jsonb)",
        )
        .bind(recoverable_id)
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::now_v7())
        .execute(&owner_pool)
        .await
        .unwrap();
        let lease_started = test_clock.now();
        let abandoned = payment_repository
            .claim_outbox(
                first_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned[0].id, recoverable_id);
        assert_eq!(abandoned[0].attempts, 1);
        let not_stale = payment_repository
            .claim_outbox(
                second_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert!(not_stale.is_empty());
        let recovered_at = lease_started + time::Duration::seconds(61);
        let recovered = payment_repository
            .claim_outbox(
                second_worker,
                1,
                recovered_at,
                recovered_at - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(recovered[0].id, recoverable_id);
        assert_eq!(recovered[0].attempts, 2);
        payment_repository
            .finish_outbox(second_worker, recoverable_id, Ok(()), recovered_at)
            .await
            .unwrap();
        assert!(
            payment_repository
                .finish_outbox(first_worker, recoverable_id, Ok(()), recovered_at)
                .await
                .is_err()
        );

        let recoverable_webhook_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.webhook_inbox \
             (id, merchant_account_id, store_id, provider, provider_event_id, event_type, \
              external_account_reference, payload, available_at, verified_at) \
             VALUES ($1, $2, $3, 'testpay', $4, 'payment.authorized', $5, '{}'::jsonb, $6, $6)",
        )
        .bind(recoverable_webhook_id)
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(format!("recoverable-{suffix}"))
        .bind(&provider_account_reference)
        .bind(lease_started)
        .execute(&owner_pool)
        .await
        .unwrap();
        let abandoned_webhook = payment_repository
            .claim_webhooks(
                first_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned_webhook[0].id, recoverable_webhook_id);
        let recovered_webhook = payment_repository
            .claim_webhooks(
                second_worker,
                1,
                recovered_at,
                recovered_at - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(recovered_webhook[0].id, recoverable_webhook_id);
        assert_eq!(recovered_webhook[0].attempts, 2);
        payment_repository
            .finish_webhook(second_worker, recoverable_webhook_id, Ok(()), recovered_at)
            .await
            .unwrap();

        let dead_letter_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.outbox_events \
             (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, $3, 'payment_attempt', $4, 'payment.create_requested', '{}'::jsonb)",
        )
        .bind(dead_letter_id)
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::now_v7())
        .execute(&owner_pool)
        .await
        .unwrap();
        for attempt in 1..=8 {
            let claim_time = test_clock.now();
            sqlx::query("UPDATE integration.outbox_events SET available_at = $2 WHERE id = $1")
                .bind(dead_letter_id)
                .bind(claim_time)
                .execute(&owner_pool)
                .await
                .unwrap();
            let worker_id = Uuid::now_v7();
            let jobs = payment_repository
                .claim_outbox(
                    worker_id,
                    1,
                    claim_time,
                    claim_time - time::Duration::minutes(1),
                )
                .await
                .unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].attempts, attempt);
            payment_repository
                .finish_outbox(
                    worker_id,
                    dead_letter_id,
                    Err("provider unavailable".into()),
                    test_clock.now(),
                )
                .await
                .unwrap();
        }
        let dead_letter: (String, i32) = sqlx::query_as(
            "SELECT status::text, attempts FROM integration.outbox_events WHERE id = $1",
        )
        .bind(dead_letter_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(dead_letter, ("dead_letter".into(), 8));

        let fulfillment_base = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/orders/{order_id}/fulfillments",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let fulfillment = app
            .clone()
            .oneshot(request(
                Method::POST,
                &fulfillment_base,
                Some("fulfillment-create"),
                Some(json!({
                    "allocations": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 1
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(fulfillment.status(), StatusCode::CREATED);
        let fulfillment = response_json(fulfillment).await;
        let fulfillment_id = fulfillment["data"]["id"].as_str().unwrap();
        assert_eq!(fulfillment["data"]["status"], "pending");

        let excessive_fulfillment = app
            .clone()
            .oneshot(request(
                Method::POST,
                &fulfillment_base,
                Some("fulfillment-excessive"),
                Some(json!({
                    "allocations": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 2
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(excessive_fulfillment.status(), StatusCode::CONFLICT);

        let fulfillment_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/fulfillments/{fulfillment_id}",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let shipped = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{fulfillment_uri}/ship"),
                Some("fulfillment-ship"),
                Some(json!({
                    "carrier": "test",
                    "tracking_number": format!("track-{suffix}")
                })),
            ))
            .await
            .unwrap();
        assert_eq!(shipped.status(), StatusCode::OK);
        assert_eq!(response_json(shipped).await["data"]["status"], "shipped");
        let delivered = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{fulfillment_uri}/deliver"),
                Some("fulfillment-deliver"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(delivered.status(), StatusCode::OK);
        assert_eq!(
            response_json(delivered).await["data"]["status"],
            "delivered"
        );

        let return_base = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/orders/{order_id}/returns",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let returned = app
            .clone()
            .oneshot(request(
                Method::POST,
                &return_base,
                Some("return-create"),
                Some(json!({
                    "lines": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 1
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(returned.status(), StatusCode::CREATED);
        let returned = response_json(returned).await;
        let return_id = returned["data"]["id"].as_str().unwrap();
        let return_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/returns/{return_id}",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        for (operation, key, body, expected) in [
            ("authorize", "return-authorize", json!({}), "authorized"),
            (
                "receive",
                "return-receive",
                json!({
                    "receipt": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "disposition": "restock",
                        "inventory_location_id": location_id.as_uuid()
                    }]
                }),
                "received",
            ),
            ("complete", "return-complete", json!({}), "completed"),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::POST,
                    &format!("{return_uri}/{operation}"),
                    Some(key),
                    Some(body),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["data"]["status"], expected);
        }

        let _ = payment_workers
            .run_outbox_batch(Uuid::now_v7(), test_clock.now(), 100)
            .await;
        let return_event_status: String = sqlx::query_scalar(
            "SELECT status::text FROM integration.outbox_events WHERE aggregate_type = 'return' \
             AND aggregate_id = $1 AND event_type = 'return.completed'",
        )
        .bind(Uuid::parse_str(return_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(return_event_status, "pending");
        let return_backlog: (Option<String>, i64, i64) = sqlx::query_as(
            "SELECT consumer_owner, pending, processed \
             FROM integration.event_consumer_backlog() WHERE event_type = 'return.completed'",
        )
        .fetch_one(&runtime_pool)
        .await
        .unwrap();
        assert_eq!(return_backlog.0, None);
        assert!(return_backlog.1 >= 1);
        assert_eq!(return_backlog.2, 0);
        let registered_owners: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT event_type, consumer_owner FROM integration.event_consumer_registry \
             ORDER BY event_type",
        )
        .fetch_all(&runtime_pool)
        .await
        .unwrap();
        assert_eq!(registered_owners.len(), 7);
        assert!(registered_owners.contains(&(
            "payment.create_requested".into(),
            Some("payments.provider_dispatch".into())
        )));
        assert!(registered_owners.contains(&(
            "search.product.changed".into(),
            Some("search.product_indexer".into())
        )));
        assert!(registered_owners.contains(&("fulfillment.shipped".into(), None)));
        assert!(
            sqlx::query(
                "UPDATE integration.event_consumer_registry \
                 SET consumer_owner = 'unauthorized.owner' \
                 WHERE event_type = 'return.completed'",
            )
            .execute(&runtime_pool)
            .await
            .is_err()
        );
        let restocks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM inventory.stock_ledger_entries WHERE kind = 'return_restock' \
             AND merchant_account_id = $1 AND store_id = $2",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(restocks, 1);

        let stock: (i64, i64) = sqlx::query_as(
            "SELECT on_hand_quantity, reserved_quantity FROM inventory.stock_items \
             WHERE merchant_account_id = $1 AND store_id = $2 AND product_variant_id = $3",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(variant_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stock, (1, 0));
        let reservation_status: String = sqlx::query_scalar(
            "SELECT status::text FROM inventory.inventory_reservations \
             WHERE merchant_account_id = $1 AND store_id = $2",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(reservation_status, "consumed");
    }
}
