use std::sync::Arc;

use chaos_application::{
    ports::{CheckoutExpiryQueue, IdempotencyRequest},
    sales::{
        CheckoutContactInput, CreateCartInput, CreateCheckoutInput, CreateOrderInput,
        PostalAddressInput, SetCartLineInput, StorefrontSales,
    },
};
use chaos_domain::{
    catalog::{ProductId, ProductVariantId},
    identity::UserId,
    inventory::InventoryLocationId,
    store::{PublishableKeyId, SalesChannelId, StoreId},
};
use sqlx::postgres::PgPoolOptions;
use time::Duration;

use super::*;

fn request(key: impl Into<String>, fingerprint: u8) -> IdempotencyRequest {
    IdempotencyRequest {
        key: key.into(),
        request_fingerprint: [fingerprint; 32],
    }
}

fn contact_input() -> CheckoutContactInput {
    CheckoutContactInput {
        email: "guest@example.com".into(),
        phone: Some("+14155552671".into()),
    }
}

fn address_input() -> PostalAddressInput {
    PostalAddressInput {
        full_name: "Guest Buyer".into(),
        company: None,
        address_line1: "1 Market Street".into(),
        address_line2: None,
        locality: "San Francisco".into(),
        administrative_area: Some("CA".into()),
        postal_code: Some("94105".into()),
        country_code: "US".into(),
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL with migrations applied"]
async fn cart_checkout_is_idempotent_atomic_isolated_and_inventory_safe() {
    let database_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let owner_pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await
        .unwrap();
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET ROLE chaos_runtime")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await
        .unwrap();
    let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();
    let user_id = UserId::new();
    let store_id = StoreId::new();
    let other_store_id = StoreId::new();
    let channel_id = SalesChannelId::new();
    let other_channel_id = SalesChannelId::new();
    let product_id = ProductId::new();
    let variant_id = ProductVariantId::new();
    let price_list_id = PriceListId::new();
    let location_id = InventoryLocationId::new();
    let inventory_item_id = Uuid::now_v7();

    sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
        .bind(user_id.as_uuid())
        .bind(format!("sales-repository-{suffix}@example.com"))
        .execute(&owner_pool)
        .await
        .unwrap();
    for (id, code) in [
        (store_id, format!("sales-{suffix}")),
        (other_store_id, format!("other-sales-{suffix}")),
    ] {
        sqlx::query(
            "INSERT INTO commerce.stores \
                 (id, code, name, status) \
                 VALUES ($1, $2, 'Sales Store', 'active')",
        )
        .bind(id.as_uuid())
        .bind(code)
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_currencies \
                 (store_id, currency) VALUES ($1, 'USD')",
        )
        .bind(id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO commerce.promotions \
             (id, store_id, handle, name, trigger, value_kind, \
              rate_basis_points, currency, minimum_subtotal_amount_minor, priority) \
             VALUES ($1, $2, 'automatic-ten', 'Automatic ten percent', 'automatic', \
                     'percentage', 1000, 'USD', 0, 100)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    let shipping_service_id = ShippingServiceId::new();
    sqlx::query(
        "INSERT INTO commerce.shipping_services \
             (id, store_id, code, name, amount_minor, currency, \
              estimated_min_days, estimated_max_days) \
             VALUES ($1, $2, 'standard', 'Standard shipping', 0, 'USD', 2, 5)",
    )
    .bind(shipping_service_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.shipping_service_regions \
             (store_id, shipping_service_id, country_code) \
             VALUES ($1, $2, 'US')",
    )
    .bind(store_id.as_uuid())
    .bind(shipping_service_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.tax_rules \
             (id, store_id, code, name, country_code, rate_basis_points) \
             VALUES ($1, $2, 'us-sales-tax', 'US sales tax', 'US', 900)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    for (id, store, code) in [
        (channel_id, store_id, "web"),
        (other_channel_id, other_store_id, "other-web"),
    ] {
        sqlx::query(
            "INSERT INTO commerce.sales_channels \
                 (id, store_id, code, name, is_default) \
                 VALUES ($1, $2, $3, 'Web', true)",
        )
        .bind(id.as_uuid())
        .bind(store.as_uuid())
        .bind(code)
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO commerce.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'checkout-product', 'Checkout Product', 'active')",
    )
    .bind(product_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title, sku, status, track_inventory) \
             VALUES ($1, $2, $3, 'Default', 'CHECKOUT-SKU', 'active', true)",
    )
    .bind(variant_id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    for locale in ["zh", "zh-CN"] {
        sqlx::query(
            "INSERT INTO commerce.store_locales \
                 (store_id, locale, created_by_user_id, created_at) \
                 VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
        )
        .bind(store_id.as_uuid())
        .bind(locale)
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    sqlx::query(
            "INSERT INTO commerce.product_translations \
             (store_id, product_id, locale, title, description, \
              updated_by_user_id, created_at, updated_at) \
             VALUES ($1, $2, 'zh', 'Localized Checkout Product', '', $3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
            "INSERT INTO commerce.product_variant_translations \
             (store_id, product_id, product_variant_id, locale, title, \
              updated_by_user_id, created_at, updated_at) \
             VALUES ($1, $2, $3, 'zh', 'Localized Default', $4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(variant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO commerce.product_publications \
             (store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3)",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(channel_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.price_lists \
             (id, store_id, code, name, currency, status) \
             VALUES ($1, $2, 'default-usd', 'Default USD', 'USD', 'active')",
    )
    .bind(price_list_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.prices \
             (id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, 1250)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(price_list_id.as_uuid())
    .bind(variant_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.inventory_locations \
             (id, store_id, code, name) \
             VALUES ($1, $2, 'primary', 'Primary')",
    )
    .bind(location_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.inventory_items \
             (id, store_id, inventory_location_id, product_variant_id, \
              on_hand_quantity) VALUES ($1, $2, $3, $4, 5)",
    )
    .bind(inventory_item_id)
    .bind(store_id.as_uuid())
    .bind(location_id.as_uuid())
    .bind(variant_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();

    let machine = MachineActor {
        publishable_key_id: PublishableKeyId::new(),
        store_id,
        sales_channel_id: Some(channel_id),
        created_by_user_id: user_id,
    };
    let actor = ShopperActor {
        machine,
        shopper_id: ShopperId::new(),
    };
    sqlx::query("INSERT INTO commerce.shoppers (id, store_id) VALUES ($1, $2)")
        .bind(actor.shopper_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    let repository = Arc::new(PostgresStorefrontSalesRepository::new(runtime_pool.clone()));
    let service = Arc::new(StorefrontSales::new(repository.clone()));
    let cart = service
        .create_cart(CreateCartInput {
            actor: actor.clone(),
            currency: None,
            locale: Some("zh-CN".into()),
            idempotency: request(format!("create-cart-{suffix}"), 1),
        })
        .await
        .unwrap();
    assert!(cart.lines.is_empty());
    assert_eq!(cart.locale.as_str(), "zh-CN");
    let unrelated_shopper = ShopperActor {
        machine: actor.machine.clone(),
        shopper_id: ShopperId::new(),
    };
    assert!(service.get_cart(&unrelated_shopper, cart.id).await.is_err());
    let updated = service
        .set_cart_line(SetCartLineInput {
            actor: actor.clone(),
            cart_id: cart.id,
            product_variant_id: variant_id,
            quantity: 3,
            idempotency: request(format!("set-line-{suffix}"), 2),
        })
        .await
        .unwrap();
    assert_eq!(updated.subtotal_amount_minor, 3_750);
    assert_eq!(updated.version, 1);
    assert_eq!(updated.lines[0].product_title, "Localized Checkout Product");
    assert_eq!(updated.lines[0].variant_title, "Localized Default");
    let replayed_creation = service
        .create_cart(CreateCartInput {
            actor: actor.clone(),
            currency: None,
            locale: Some("zh-CN".into()),
            idempotency: request(format!("create-cart-{suffix}"), 1),
        })
        .await
        .unwrap();
    assert!(replayed_creation.lines.is_empty());
    assert_eq!(replayed_creation.version, 0);

    sqlx::query(
        "UPDATE commerce.product_translations SET title = 'Changed Translation' \
             WHERE store_id = $1 AND product_id = $2 \
               AND locale = 'zh'",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();

    let now = OffsetDateTime::now_utc();
    let first_service = service.clone();
    let first_actor = actor.clone();
    let first_key = format!("checkout-{suffix}");
    let first = tokio::spawn(async move {
        first_service
            .create_checkout(CreateCheckoutInput {
                actor: first_actor,
                cart_id: cart.id,
                contact: contact_input(),
                billing_address: address_input(),
                shipping_address: Some(address_input()),
                shipping_service_id: Some(shipping_service_id),
                promotion_code: None,
                now,
                idempotency: request(first_key, 3),
            })
            .await
    });
    let second_service = service.clone();
    let second_actor = actor.clone();
    let second_key = format!("checkout-{suffix}");
    let second = tokio::spawn(async move {
        second_service
            .create_checkout(CreateCheckoutInput {
                actor: second_actor,
                cart_id: cart.id,
                contact: contact_input(),
                billing_address: address_input(),
                shipping_address: Some(address_input()),
                shipping_service_id: Some(shipping_service_id),
                promotion_code: None,
                now,
                idempotency: request(second_key, 3),
            })
            .await
    });
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.discount_amount_minor, 375);
    assert_eq!(first.tax_amount_minor, 304);
    assert_eq!(first.total_amount_minor, 3_679);
    assert_eq!(first.promotion.as_ref().unwrap().handle(), "automatic-ten");
    assert_eq!(first.locale.as_str(), "zh-CN");
    assert_eq!(first.lines[0].product_title, "Localized Checkout Product");
    assert!(first.inventory_reservation_id.is_some());
    assert_eq!(first.identity.contact().email(), "guest@example.com");
    assert_eq!(
        first.identity.shipping_address().unwrap().country_code(),
        "US"
    );

    let stock: (i64, i64) = sqlx::query_as(
        "SELECT on_hand_quantity, reserved_quantity \
             FROM commerce.inventory_items WHERE id = $1",
    )
    .bind(inventory_item_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(stock, (5, 3));
    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce.inventory_transactions \
             WHERE reference_type = 'reservation' AND reference_id = $1",
    )
    .bind(first.inventory_reservation_id.unwrap().as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 1);

    let order = service
        .create_order(CreateOrderInput {
            actor: actor.clone(),
            checkout_id: first.id,
            now,
            idempotency: request(format!("create-order-{suffix}"), 4),
        })
        .await
        .unwrap();
    assert!(order.order_number.as_str().starts_with("W-"));
    assert_eq!(order.order_number.as_str().len(), 19);

    let tracking_key = SecretString::from(format!("otk_{}", "A".repeat(43)));
    let tracking_digest: [u8; 32] = Sha256::digest(tracking_key.expose_secret()).into();
    sqlx::query(
        "INSERT INTO commerce.order_tracking_keys \
             (id,store_id,order_id,secret_digest,expires_at,created_at) \
             VALUES($1,$2,$3,$4,$5,$6)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(order.id.as_uuid())
    .bind(tracking_digest.as_slice())
    .bind(now + time::Duration::days(180))
    .bind(now)
    .execute(&owner_pool)
    .await
    .unwrap();
    let tracking_session = service
        .exchange_order_tracking_key(&actor.machine, &tracking_key, now)
        .await
        .unwrap();
    assert_eq!(tracking_session.order.id, order.id);
    let tracked = service
        .get_tracked_order(&actor.machine, &tracking_session.access_token, now)
        .await
        .unwrap();
    assert_eq!(tracked.order_number, order.order_number);
    assert!(
        service
            .exchange_order_tracking_key(
                &actor.machine,
                &SecretString::from(format!("otk_{}", "B".repeat(43))),
                now,
            )
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE commerce.checkouts SET status='pending',closed_at=NULL,updated_at=$1 \
             WHERE store_id=$2 AND id=$3",
    )
    .bind(now)
    .bind(store_id.as_uuid())
    .bind(first.id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();

    let other_actor = ShopperActor {
        machine: MachineActor {
            store_id: other_store_id,
            sales_channel_id: Some(other_channel_id),
            ..actor.machine.clone()
        },
        shopper_id: actor.shopper_id,
    };
    assert!(service.get_cart(&other_actor, cart.id).await.is_err());
    assert!(
        service
            .create_checkout(CreateCheckoutInput {
                actor,
                cart_id: cart.id,
                contact: contact_input(),
                billing_address: address_input(),
                shipping_address: Some(address_input()),
                shipping_service_id: Some(shipping_service_id),
                promotion_code: None,
                now,
                idempotency: request(format!("second-checkout-{suffix}"), 5),
            })
            .await
            .is_err()
    );

    let mut runtime_connection = runtime_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.store_id', $1, false)")
        .bind(store_id.as_uuid().to_string())
        .execute(&mut *runtime_connection)
        .await
        .unwrap();
    assert!(
        sqlx::query(
            "UPDATE commerce.checkout_lines SET product_title = 'Tampered' \
                 WHERE checkout_id = $1",
        )
        .bind(first.id.as_uuid())
        .execute(&mut *runtime_connection)
        .await
        .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE commerce.checkout_contacts SET email = 'tampered@example.com' \
                 WHERE checkout_id = $1",
        )
        .bind(first.id.as_uuid())
        .execute(&mut *runtime_connection)
        .await
        .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE commerce.checkout_addresses SET address_line1 = 'Tampered' \
                 WHERE checkout_id = $1",
        )
        .bind(first.id.as_uuid())
        .execute(&mut *runtime_connection)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM commerce.checkouts WHERE id = $1")
            .bind(first.id.as_uuid())
            .execute(&mut *runtime_connection)
            .await
            .is_err()
    );

    let reservation_id = first.inventory_reservation_id.unwrap();
    let initial_expiry_worker = Uuid::now_v7();
    let first_expiry_attempt = first.expires_at + Duration::seconds(1);
    let jobs = repository
        .claim_due_checkouts(
            initial_expiry_worker,
            10,
            first_expiry_attempt,
            first_expiry_attempt - Duration::minutes(1),
        )
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, first.id);
    assert!(
        repository
            .claim_due_checkouts(
                Uuid::now_v7(),
                10,
                first_expiry_attempt,
                first_expiry_attempt - Duration::minutes(1),
            )
            .await
            .unwrap()
            .is_empty()
    );

    let recovery_worker = Uuid::now_v7();
    let recovery_time = first_expiry_attempt + Duration::minutes(1);
    let recovered = repository
        .claim_due_checkouts(
            recovery_worker,
            10,
            recovery_time,
            recovery_time - Duration::minutes(1),
        )
        .await
        .unwrap();
    assert_eq!(recovered, jobs);
    assert!(
        repository
            .expire_checkout(initial_expiry_worker, jobs[0], recovery_time)
            .await
            .is_err()
    );
    repository
        .expire_checkout(recovery_worker, recovered[0], recovery_time)
        .await
        .unwrap();

    let checkout_state: (
        String,
        Option<OffsetDateTime>,
        Option<Uuid>,
        Option<OffsetDateTime>,
    ) = sqlx::query_as(
        "SELECT status::text, closed_at, expiry_locked_by, expiry_locked_at \
                 FROM commerce.checkouts WHERE id = $1",
    )
    .bind(first.id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        checkout_state,
        ("expired".into(), Some(recovery_time), None, None)
    );
    let reservation_state: (String, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT status::text, closed_at FROM commerce.inventory_reservations WHERE id = $1",
    )
    .bind(reservation_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(reservation_state, ("expired".into(), Some(recovery_time)));
    let released_stock: (i64, i64) = sqlx::query_as(
        "SELECT on_hand_quantity, reserved_quantity FROM commerce.inventory_items WHERE id = $1",
    )
    .bind(inventory_item_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(released_stock, (5, 0));
    let expiry_ledger_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce.inventory_transactions \
             WHERE reference_type = 'reservation' AND reference_id = $1 \
               AND reserved_delta_quantity < 0",
    )
    .bind(reservation_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(expiry_ledger_count, 1);
    assert!(
        repository
            .claim_due_checkouts(
                Uuid::now_v7(),
                10,
                recovery_time,
                recovery_time - Duration::minutes(1),
            )
            .await
            .unwrap()
            .is_empty()
    );
}
