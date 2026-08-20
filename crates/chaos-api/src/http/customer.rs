use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    ports::{CustomerAddressDetail, CustomerDetail, IdempotencyRequest},
    sales::{
        AssociateCustomerInput, CreateCustomerAddressInput, DeleteCustomerAddressInput,
        UpdateCustomerInput,
    },
};
use chaos_domain::sales::CustomerAddressId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, CartShopper,
    CustomerCheckout, CustomerMachine, CustomerSession,
    pagination::{
        CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta,
    },
    storefront_sales::{OrderData, order_data},
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/customer/associate", post(associate))
        .route("/customer", get(get_customer).put(update_customer))
        .route("/customer/addresses", post(create_address))
        .route(
            "/customer/addresses/{address_id}",
            axum::routing::delete(delete_address),
        )
        .route("/customer/orders", get(list_orders))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateCustomerBody {
    phone: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateAddressBody {
    label: String,
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}

#[derive(Deserialize)]
struct AddressPath {
    address_id: Uuid,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct CustomerData {
    id: Uuid,
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    addresses: Vec<CustomerAddressData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct CustomerAddressData {
    id: Uuid,
    label: String,
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
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct CustomerAddressMutationData {
    customer_id: Uuid,
}

async fn associate(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(shopper): CartShopper,
    CustomerSession(user_id): CustomerSession,
) -> Result<ApiResponse<CustomerData>, ApiError> {
    let idempotency = request(&headers, "associate_customer", &user_id.as_uuid())?;
    let detail = state
        .customer_service
        .associate(AssociateCustomerInput {
            shopper,
            user_id,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(customer_data(detail)))
}

async fn get_customer(
    State(state): State<ApiState>,
    CustomerMachine(actor): CustomerMachine,
) -> Result<ApiResponse<CustomerData>, ApiError> {
    let detail = state
        .customer_service
        .get(&actor)
        .await?
        .ok_or_else(customer_not_found)?;
    Ok(ApiResponse::ok(customer_data(detail)))
}

async fn update_customer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CustomerMachine(actor): CustomerMachine,
    ApiJson(body): ApiJson<UpdateCustomerBody>,
) -> Result<ApiResponse<CustomerData>, ApiError> {
    let idempotency = request(&headers, "update_customer", &body)?;
    let detail = state
        .customer_service
        .update(UpdateCustomerInput {
            actor,
            phone: body.phone,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(customer_data(detail)))
}

async fn create_address(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CustomerMachine(actor): CustomerMachine,
    ApiJson(body): ApiJson<CreateAddressBody>,
) -> Result<ApiResponse<CustomerAddressData>, ApiError> {
    let idempotency = request(&headers, "create_customer_address", &body)?;
    let detail = state
        .customer_service
        .create_address(CreateCustomerAddressInput {
            actor,
            label: body.label,
            address: chaos_application::sales::PostalAddressInput {
                full_name: body.full_name,
                company: body.company,
                address_line1: body.address_line1,
                address_line2: body.address_line2,
                locality: body.locality,
                administrative_area: body.administrative_area,
                postal_code: body.postal_code,
                country_code: body.country_code,
            },
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(address_data(detail)))
}

async fn delete_address(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CustomerMachine(actor): CustomerMachine,
    ApiPath(path): ApiPath<AddressPath>,
) -> Result<ApiResponse<CustomerAddressMutationData>, ApiError> {
    let idempotency = request(&headers, "delete_customer_address", &path.address_id)?;
    let customer_id = state
        .customer_service
        .delete_address(DeleteCustomerAddressInput {
            actor,
            address_id: CustomerAddressId::from_uuid(path.address_id),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(CustomerAddressMutationData {
        customer_id: customer_id.as_uuid(),
    }))
}

async fn list_orders(
    State(state): State<ApiState>,
    CustomerCheckout(actor): CustomerCheckout,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<OrderData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::CustomerOrder))
        .transpose()?;
    let page = state
        .customer_service
        .list_orders(&actor, after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|order| encode_cursor(order.id.as_uuid(), CursorKind::CustomerOrder))
        })
        .flatten();
    let data = page
        .items
        .into_iter()
        .map(order_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor)))
}

fn customer_data(detail: CustomerDetail) -> CustomerData {
    CustomerData {
        id: detail.customer.id().as_uuid(),
        email: detail.customer.contact().email().into(),
        phone: detail.customer.contact().phone().map(Into::into),
        addresses: detail.addresses.into_iter().map(address_data).collect(),
        created_at: detail.created_at.into(),
        updated_at: detail.updated_at.into(),
    }
}

fn address_data(detail: CustomerAddressDetail) -> CustomerAddressData {
    let address = detail.address.address();
    CustomerAddressData {
        id: detail.address.id().as_uuid(),
        label: detail.address.label().into(),
        full_name: address.full_name().into(),
        company: address.company().map(Into::into),
        address_line1: address.address_line1().into(),
        address_line2: address.address_line2().map(Into::into),
        locality: address.locality().into(),
        administrative_area: address.administrative_area().map(Into::into),
        postal_code: address.postal_code().map(Into::into),
        country_code: address.country_code().into(),
        created_at: detail.created_at.into(),
        updated_at: detail.updated_at.into(),
    }
}

fn request<T: Serialize>(
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

fn customer_not_found() -> ApplicationError {
    ApplicationError::NotFound {
        resource: "customer",
        id: "current".into(),
    }
}
