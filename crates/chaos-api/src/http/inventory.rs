use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    inventory::{AdjustStockInput, CreateInventoryLocationInput},
    ports::{AdminActor, IdempotencyRequest, InventoryLocationItem, StockItemItem},
};
use chaos_domain::{
    catalog::ProductVariantId,
    inventory::{InventoryLocationId, StockItemId},
    merchant::StoreId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, StoreContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/stores/{store_id}/inventory-locations",
            get(list_locations).post(create_location),
        )
        .route("/stores/{store_id}/inventory-items", get(list_stock))
        .route(
            "/stores/{store_id}/inventory-adjustments",
            post(adjust_stock),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateLocationBody {
    code: String,
    name: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdjustStockBody {
    inventory_location_id: Uuid,
    product_variant_id: Uuid,
    delta_quantity: i64,
    note: String,
}

#[derive(Serialize)]
struct MutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct LocationData {
    id: Uuid,
    code: String,
    name: String,
    status: &'static str,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct StockItemData {
    id: Uuid,
    inventory_location_id: Uuid,
    product_variant_id: Uuid,
    on_hand_quantity: i64,
    reserved_quantity: i64,
    available_quantity: i64,
    updated_at: ApiDateTime,
}

async fn create_location(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateLocationBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let idempotency = body_request(&headers, path.store_id, &body)?;
    let id = state
        .inventory_management
        .create_location(CreateInventoryLocationInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

async fn list_locations(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<LocationData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::InventoryLocation))
        .transpose()?
        .map(InventoryLocationId::from_uuid);
    let page = state
        .inventory_management
        .list_locations(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::InventoryLocation))
    });
    let data = page
        .items
        .into_iter()
        .map(location_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn list_stock(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<StockItemData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::StockItem))
        .transpose()?
        .map(StockItemId::from_uuid);
    let page = state
        .inventory_management
        .list_stock(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::StockItem))
    });
    let data = page
        .items
        .into_iter()
        .map(stock_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn adjust_stock(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<AdjustStockBody>,
) -> Result<ApiResponse<StockItemData>, ApiError> {
    let idempotency = body_request(&headers, path.store_id, &body)?;
    let item = state
        .inventory_management
        .adjust_stock(AdjustStockInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            inventory_location_id: InventoryLocationId::from_uuid(body.inventory_location_id),
            product_variant_id: ProductVariantId::from_uuid(body.product_variant_id),
            delta_quantity: body.delta_quantity,
            note: body.note,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(stock_data(item)?))
}

fn body_request<T: Serialize>(
    headers: &HeaderMap,
    store_id: Uuid,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(store_id, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn location_data(item: InventoryLocationItem) -> Result<LocationData, ApplicationError> {
    Ok(LocationData {
        id: item.id.as_uuid(),
        code: item.code,
        name: item.name,
        status: item.status.as_str(),
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    })
}

fn stock_data(item: StockItemItem) -> Result<StockItemData, ApplicationError> {
    Ok(StockItemData {
        id: item.id.as_uuid(),
        inventory_location_id: item.inventory_location_id.as_uuid(),
        product_variant_id: item.product_variant_id.as_uuid(),
        on_hand_quantity: item.on_hand_quantity,
        reserved_quantity: item.reserved_quantity,
        available_quantity: item.available_quantity,
        updated_at: item.updated_at.into(),
    })
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        merchant::StoreId,
    };
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn inventory_http_matrix_covers_crud_pagination_and_errors() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner_id = UserId::new();
        let support_id = UserId::new();
        let outsider_id = UserId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, label) in [
            (owner_id, "owner"),
            (support_id, "support"),
            (outsider_id, "outsider"),
        ] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("inventory-http-{label}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        for (id, code) in [
            (store_id, format!("inv-{}", &suffix[12..28])),
            (other_store_id, format!("invo-{}", &suffix[12..28])),
        ] {
            sqlx::query(
                    "INSERT INTO merchant.stores (id, code, name, status) VALUES ($1, $2, 'Inventory HTTP Test', 'active')",
                 )
            .bind(id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, role) in [(owner_id, "owner"), (support_id, "member")] {
            sqlx::query(
                "INSERT INTO merchant.store_memberships \
                 (store_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.store_role)",
            )
            .bind(store_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'inventory-http-product', 'Inventory Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, store_id, product_id, title, status, track_inventory) \
             VALUES ($1, $2, $3, 'Default', 'active', true)",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let base_uri = format!("/admin/v1/stores/{}", store_id.as_uuid());
        let locations_uri = format!("{base_uri}/inventory-locations");
        let adjustments_uri = format!("{base_uri}/inventory-adjustments");
        let items_uri = format!("{base_uri}/inventory-items");
        let owner_state = test_state(&database_url, owner_id);

        let response = router(owner_state.clone())
            .oneshot(Request::get(&locations_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &locations_uri,
                Some(&format!("location-primary-{suffix}")),
                Some(json!({ "code": "primary", "name": "Primary Warehouse" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let location_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &locations_uri,
                Some(&format!("location-secondary-{suffix}")),
                Some(json!({ "code": "secondary", "name": "Secondary Warehouse" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                &format!("{locations_uri}?limit=1"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first_page = response_json(response).await;
        assert_eq!(first_page["data"].as_array().unwrap().len(), 1);
        assert_eq!(first_page["meta"]["page"]["has_more"], true);
        let cursor = first_page["meta"]["page"]["next_cursor"].as_str().unwrap();
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                &format!("{locations_uri}?limit=1&cursor={cursor}"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["meta"]["page"]["has_more"],
            false
        );

        let adjustment = json!({
            "inventory_location_id": location_id,
            "product_variant_id": variant_id.as_uuid(),
            "delta_quantity": 10,
            "note": "Initial receipt"
        });
        for _ in 0..2 {
            let response = router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &adjustments_uri,
                    Some(&format!("adjust-inventory-{suffix}")),
                    Some(adjustment.clone()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let data = response_json(response).await;
            assert_eq!(data["data"]["on_hand_quantity"], 10);
            assert_eq!(data["data"]["available_quantity"], 10);
        }
        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &items_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &adjustments_uri,
                Some(&format!("invalid-adjust-{suffix}")),
                Some(json!({
                    "inventory_location_id": location_id,
                    "product_variant_id": variant_id.as_uuid(),
                    "delta_quantity": 0,
                    "note": "No change"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &locations_uri,
                Some(&format!("duplicate-location-{suffix}")),
                Some(json!({ "code": "primary", "name": "Duplicate" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let support_state = test_state(&database_url, support_id);
        let response = router(support_state)
            .oneshot(request(
                Method::POST,
                &locations_uri,
                Some(&format!("support-location-{suffix}")),
                Some(json!({ "code": "support", "name": "Allowed" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                &format!("/admin/v1/stores/{}/inventory-items", Uuid::now_v7()),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let outsider_state = test_state(&database_url, outsider_id);
        let response = router(outsider_state)
            .oneshot(request(
                Method::GET,
                &format!("/admin/v1/stores/{}/inventory-items", store_id.as_uuid()),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
