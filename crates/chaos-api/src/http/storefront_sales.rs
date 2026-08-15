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
        CreateCartInput, CreateCheckoutInput, CreateOrderInput, RemoveCartLineInput,
        SetCartLineInput,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    sales::{CartId, CheckoutId, OrderId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CartMachine, CheckoutMachine,
    merchant::idempotency_key, response::format_time,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route(
            "/carts/{cart_id}/lines/{product_variant_id}",
            axum::routing::put(set_cart_line).delete(remove_cart_line),
        )
        .route("/carts/{cart_id}/checkout", post(create_checkout))
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
    created_at: String,
    updated_at: String,
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
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    expires_at: String,
    lines: Vec<CheckoutLineData>,
    created_at: String,
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
    occurred_at: String,
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
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineData>,
    transitions: Vec<OrderTransitionData>,
    created_at: String,
    updated_at: String,
}

async fn create_cart(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartMachine(actor): CartMachine,
    ApiJson(body): ApiJson<CreateCartBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let idempotency = body_request(&headers, "create_cart", &body)?;
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput {
            actor,
            currency: body.currency,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(cart_data(cart)?))
}

async fn get_cart(
    State(state): State<ApiState>,
    CartMachine(actor): CartMachine,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let cart = state
        .storefront_sales
        .get_cart(&actor, CartId::from_uuid(path.cart_id))
        .await?;
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn set_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartMachine(actor): CartMachine,
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
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn remove_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartMachine(actor): CartMachine,
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
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn create_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutMachine(actor): CheckoutMachine,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<ApiResponse<CheckoutData>, ApiError> {
    let idempotency = body_request(&headers, "create_checkout", &path.cart_id)?;
    let checkout = state
        .storefront_sales
        .create_checkout(CreateCheckoutInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            now: OffsetDateTime::now_utc(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(checkout_data(checkout)?))
}

async fn get_checkout(
    State(state): State<ApiState>,
    CheckoutMachine(actor): CheckoutMachine,
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
    CheckoutMachine(actor): CheckoutMachine,
    ApiPath(path): ApiPath<CheckoutPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let idempotency = body_request(&headers, "create_order", &path.checkout_id)?;
    let order = state
        .storefront_sales
        .create_order(CreateOrderInput {
            actor,
            checkout_id: CheckoutId::from_uuid(path.checkout_id),
            now: OffsetDateTime::now_utc(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(order_data(order)?))
}

async fn get_order(
    State(state): State<ApiState>,
    CheckoutMachine(actor): CheckoutMachine,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let order = state
        .storefront_sales
        .get_order(&actor, OrderId::from_uuid(path.order_id))
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
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

fn cart_data(cart: CartDetail) -> Result<CartData, ApplicationError> {
    Ok(CartData {
        id: cart.id.as_uuid(),
        price_list_id: cart.price_list_id.as_uuid(),
        currency: cart.currency.as_str().to_owned(),
        status: cart.status.as_str(),
        version: cart.version,
        lines: cart.lines.into_iter().map(cart_line_data).collect(),
        subtotal_amount_minor: cart.subtotal_amount_minor,
        created_at: format_time(cart.created_at)?,
        updated_at: format_time(cart.updated_at)?,
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
        subtotal_amount_minor: checkout.subtotal_amount_minor,
        discount_amount_minor: checkout.discount_amount_minor,
        tax_amount_minor: checkout.tax_amount_minor,
        total_amount_minor: checkout.total_amount_minor,
        expires_at: format_time(checkout.expires_at)?,
        lines: checkout.lines.into_iter().map(checkout_line_data).collect(),
        created_at: format_time(checkout.created_at)?,
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
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
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
                    occurred_at: format_time(transition.occurred_at)?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: format_time(order.created_at)?,
        updated_at: format_time(order.updated_at)?,
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
        http::{Method, Request, StatusCode},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chaos_application::ports::{ApiKeyMaterialGenerator, GeneratedApiKeyMaterial};
    use chaos_application::{payments::PaymentWorkers, ports::IntegrationQueue};
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
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
    use time::OffsetDateTime;
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
        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            state.infrastructure.runtime_pool(),
        ));
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
        let missing_idempotency = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/carts",
                Some(full_secret),
                None,
                Some(json!({})),
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
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let create = store_request(
            Method::POST,
            "/store/v1/carts",
            Some(full_secret),
            Some("create"),
            Some(json!({})),
        );
        let created = app.clone().oneshot(create).await.unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let cart_id = created["data"]["id"].as_str().unwrap();

        let line_uri = format!("/store/v1/carts/{cart_id}/lines/{}", variant_id.as_uuid());
        let invalid_line = app
            .clone()
            .oneshot(store_request(
                Method::PUT,
                &line_uri,
                Some(full_secret),
                Some("invalid-line"),
                Some(json!({"quantity": 0})),
            ))
            .await
            .unwrap();
        assert_eq!(invalid_line.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let updated = app
            .clone()
            .oneshot(store_request(
                Method::PUT,
                &line_uri,
                Some(full_secret),
                Some("line"),
                Some(json!({"quantity": 2})),
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = response_json(updated).await;
        assert_eq!(updated["data"]["subtotal_amount_minor"], 2500);

        let checkout_uri = format!("/store/v1/carts/{cart_id}/checkout");
        let checkout = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &checkout_uri,
                Some(full_secret),
                Some("checkout"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(checkout.status(), StatusCode::CREATED);
        let checkout = response_json(checkout).await;
        let checkout_id = checkout["data"]["id"].as_str().unwrap();
        assert_eq!(checkout["data"]["total_amount_minor"], 2500);
        assert_eq!(
            checkout["data"]["lines"][0]["product_title"],
            "Sales Product"
        );
        assert!(checkout["data"]["inventory_reservation_id"].is_string());

        let replay = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &checkout_uri,
                Some(full_secret),
                Some("checkout"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(replay).await["data"]["id"], checkout_id);

        let order_uri = format!("/store/v1/checkouts/{checkout_id}/order");
        let order = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &order_uri,
                Some(full_secret),
                Some("order"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(order.status(), StatusCode::CREATED);
        let order = response_json(order).await;
        let order_id = order["data"]["id"].as_str().unwrap();
        assert_eq!(order["data"]["status"], "pending");
        assert_eq!(order["data"]["total_amount_minor"], 2500);
        assert_eq!(order["data"]["transitions"][0]["kind"], "created");
        let order_replay = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &order_uri,
                Some(full_secret),
                Some("order"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(order_replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(order_replay).await["data"]["id"], order_id);

        let terminal = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &checkout_uri,
                Some(full_secret),
                Some("checkout-again"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(terminal.status(), StatusCode::CONFLICT);

        let fetched = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/checkouts/{checkout_id}"),
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_eq!(response_json(fetched).await["data"]["id"], checkout_id);

        let fetched_order = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/orders/{order_id}"),
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(fetched_order.status(), StatusCode::OK);
        assert_eq!(response_json(fetched_order).await["data"]["id"], order_id);

        let payment_attempt = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                &format!("/store/v1/orders/{order_id}/payment-attempts"),
                Some(full_secret),
                Some("payment-attempt"),
                Some(json!({"provider": "testpay"})),
            ))
            .await
            .unwrap();
        assert_eq!(payment_attempt.status(), StatusCode::CREATED);
        let payment_attempt = response_json(payment_attempt).await;
        let payment_attempt_id = payment_attempt["data"]["id"].as_str().unwrap();
        assert_eq!(payment_attempt["data"]["status"], "pending");
        let cross_store = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                Some(other_store_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(cross_store.status(), StatusCode::NOT_FOUND);

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
                    .run_webhook_batch(Uuid::now_v7(), OffsetDateTime::now_utc(), 10)
                    .await
                    .unwrap(),
                1
            );
        }

        let captured = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                Some(full_secret),
                None,
                None,
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
                .run_webhook_batch(Uuid::now_v7(), OffsetDateTime::now_utc(), 10)
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
        let claimed_at = OffsetDateTime::now_utc();
        let (first_jobs, second_jobs) = tokio::join!(
            payment_repository.claim_outbox(first_worker, 10, claimed_at),
            payment_repository.claim_outbox(second_worker, 10, claimed_at),
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
            sqlx::query(
                "UPDATE integration.outbox_events SET available_at = CURRENT_TIMESTAMP \
                 WHERE id = $1",
            )
            .bind(dead_letter_id)
            .execute(&owner_pool)
            .await
            .unwrap();
            let worker_id = Uuid::now_v7();
            let jobs = payment_repository
                .claim_outbox(worker_id, 1, OffsetDateTime::now_utc())
                .await
                .unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].attempts, attempt);
            payment_repository
                .finish_outbox(
                    worker_id,
                    dead_letter_id,
                    Err("provider unavailable".into()),
                    OffsetDateTime::now_utc(),
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

        let return_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM integration.outbox_events WHERE aggregate_type = 'return' \
             AND aggregate_id = $1 AND event_type = 'return.completed'",
        )
        .bind(Uuid::parse_str(return_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(return_events, 1);
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
