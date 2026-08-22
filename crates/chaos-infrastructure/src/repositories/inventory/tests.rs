// Database-backed inventory repository tests.

use std::sync::Arc;

use chaos_application::{
    inventory::{
        AdjustInventoryInput, CreateInventoryLocationInput, InventoryManagement,
        ReserveInventoryInput, ReserveInventoryLineInput, TransitionInventoryReservationInput,
    },
    ports::IdempotencyRequest,
    store::StoreQueries,
};
use chaos_domain::{
    catalog::{ProductId, ProductVariantId},
    identity::UserId,
    store::{PublishableKeyId, SalesChannelId},
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

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL with migrations applied"]
async fn inventory_is_concurrency_safe_isolated_and_append_only() {
    let database_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let owner_pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await
        .unwrap();
    let runtime_pool = PgPoolOptions::new()
        .max_connections(6)
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
    let other_user_id = UserId::new();
    let store_id = StoreId::new();
    let other_store_id = StoreId::new();
    let channel_id = SalesChannelId::new();
    let product_id = ProductId::new();
    let variant_id = ProductVariantId::new();

    for (id, email) in [
        (user_id, format!("inventory-{suffix}@example.com")),
        (
            other_user_id,
            format!("inventory-other-{suffix}@example.com"),
        ),
    ] {
        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(id.as_uuid())
            .bind(email)
            .execute(&owner_pool)
            .await
            .unwrap();
    }
    for (id, code) in [
        (store_id, "inventory-store"),
        (other_store_id, "other-inventory"),
    ] {
        sqlx::query(
            "INSERT INTO commerce.stores \
                 (id, code, name, status) \
                 VALUES ($1, $2, 'Inventory Store', 'active')",
        )
        .bind(id.as_uuid())
        .bind(format!("{code}-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    for (store, user) in [(store_id, user_id), (other_store_id, other_user_id)] {
        sqlx::query(
            "INSERT INTO commerce.store_memberships \
                 (store_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(store.as_uuid())
        .bind(user.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO commerce.sales_channels \
             (id, store_id, code, name, is_default) \
             VALUES ($1, $2, 'web', 'Web', true)",
    )
    .bind(channel_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'inventory-product', 'Inventory Product', 'active')",
    )
    .bind(product_id.as_uuid())
    .bind(store_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title, track_inventory) \
             VALUES ($1, $2, $3, 'Default', true)",
    )
    .bind(variant_id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();

    let queries = StoreQueries::new(Arc::new(
        crate::repositories::PostgresStoreReadRepository::new(runtime_pool.clone()),
    ));
    let actor = queries.authorize(user_id, store_id).await.unwrap();
    let other_actor = queries
        .authorize(other_user_id, other_store_id)
        .await
        .unwrap();
    let service = Arc::new(InventoryManagement::new(Arc::new(
        PostgresInventoryRepository::new(runtime_pool.clone()),
    )));
    let location_request = request(format!("location-{suffix}"), 1);
    let location_id = service
        .create_location(CreateInventoryLocationInput {
            actor: AdminActor::Store(actor),
            store_id,
            code: "primary".into(),
            name: "Primary Warehouse".into(),
            idempotency: location_request,
        })
        .await
        .unwrap();
    let adjustment_key = format!("adjust-{suffix}");
    let inventory_item = service
        .adjust_inventory_item(AdjustInventoryInput {
            actor: AdminActor::Store(actor),
            store_id,
            inventory_location_id: location_id,
            product_variant_id: variant_id,
            delta_quantity: 10,
            note: "Initial receipt".into(),
            idempotency: request(adjustment_key.clone(), 2),
        })
        .await
        .unwrap();
    let replay = service
        .adjust_inventory_item(AdjustInventoryInput {
            actor: AdminActor::Store(actor),
            store_id,
            inventory_location_id: location_id,
            product_variant_id: variant_id,
            delta_quantity: 10,
            note: "Initial receipt".into(),
            idempotency: request(adjustment_key, 2),
        })
        .await
        .unwrap();
    assert_eq!(replay.on_hand_quantity, 10);
    assert!(
        service
            .adjust_inventory_item(AdjustInventoryInput {
                actor: AdminActor::Store(actor),
                store_id,
                inventory_location_id: location_id,
                product_variant_id: variant_id,
                delta_quantity: -11,
                note: "Invalid shrinkage".into(),
                idempotency: request(format!("invalid-adjust-{suffix}"), 3),
            })
            .await
            .is_err()
    );

    let machine = MachineActor {
        publishable_key_id: PublishableKeyId::new(),
        store_id,
        sales_channel_id: Some(channel_id),
        created_by_user_id: user_id,
    };
    let now = OffsetDateTime::now_utc();
    let first_input = ReserveInventoryInput {
        actor: machine.clone(),
        now,
        expires_at: now + Duration::minutes(15),
        lines: vec![ReserveInventoryLineInput {
            inventory_item_id: inventory_item.id,
            quantity: 7,
        }],
        idempotency: request(format!("reserve-a-{suffix}"), 4),
    };
    let second_input = ReserveInventoryInput {
        actor: machine.clone(),
        now,
        expires_at: now + Duration::minutes(15),
        lines: vec![ReserveInventoryLineInput {
            inventory_item_id: inventory_item.id,
            quantity: 7,
        }],
        idempotency: request(format!("reserve-b-{suffix}"), 5),
    };
    let first_service = service.clone();
    let first = tokio::spawn(async move { first_service.reserve(first_input).await });
    let second_service = service.clone();
    let second = tokio::spawn(async move { second_service.reserve(second_input).await });
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    let reservation_id = outcomes.into_iter().find_map(Result::ok).unwrap();
    let after_reserve = service
        .list_inventory_items(AdminActor::Store(actor), store_id, None, 100)
        .await
        .unwrap();
    assert_eq!(after_reserve.items[0].reserved_quantity, 7);
    assert_eq!(after_reserve.items[0].available_quantity, 3);

    service
        .consume(TransitionInventoryReservationInput {
            actor: machine.clone(),
            reservation_id,
            now: now + Duration::minutes(1),
            idempotency: request(format!("consume-{suffix}"), 6),
        })
        .await
        .unwrap();
    let after_consume = service
        .list_inventory_items(AdminActor::Store(actor), store_id, None, 100)
        .await
        .unwrap();
    assert_eq!(after_consume.items[0].on_hand_quantity, 3);
    assert_eq!(after_consume.items[0].reserved_quantity, 0);

    let expiring_id = service
        .reserve(ReserveInventoryInput {
            actor: machine,
            now,
            expires_at: now + Duration::minutes(5),
            lines: vec![ReserveInventoryLineInput {
                inventory_item_id: inventory_item.id,
                quantity: 1,
            }],
            idempotency: request(format!("reserve-expiring-{suffix}"), 7),
        })
        .await
        .unwrap();
    assert_eq!(
        service
            .expire_due(actor, store_id, now + Duration::minutes(6), 100)
            .await
            .unwrap(),
        1
    );
    let expired_status: String = sqlx::query_scalar(
        "SELECT status::text FROM commerce.inventory_reservations WHERE id = $1",
    )
    .bind(expiring_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(expired_status, "expired");

    assert!(
        service
            .list_inventory_items(AdminActor::Store(other_actor), store_id, None, 100)
            .await
            .is_err()
    );
    assert!(
        service
            .list_inventory_items(AdminActor::Store(actor), other_store_id, None, 100)
            .await
            .is_err()
    );
    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce.inventory_transactions \
             WHERE store_id = $1",
    )
    .bind(store_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 5);

    let mut runtime_connection = runtime_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.store_id', $1, false)")
        .bind(store_id.as_uuid().to_string())
        .execute(&mut *runtime_connection)
        .await
        .unwrap();
    assert!(
        sqlx::query(
            "UPDATE commerce.inventory_transactions SET note = 'tampered' \
                 WHERE store_id = $1",
        )
        .bind(store_id.as_uuid())
        .execute(&mut *runtime_connection)
        .await
        .is_err()
    );
}
