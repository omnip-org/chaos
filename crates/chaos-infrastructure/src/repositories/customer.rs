use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        CustomerActor, CustomerAddressDetail, CustomerDetail, CustomerOrderPage,
        CustomerRepository, IdempotencyRequest, ShopperActor,
    },
};
use chaos_domain::{
    identity::UserId,
    merchant::SalesChannelId,
    sales::{Customer, CustomerAddress, CustomerAddressId, CustomerId, PostalAddress},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    idempotency::{self, IdempotencyScope},
    storefront_sales::load_order,
};

const ASSOCIATE_OPERATION: &str = "customers.associate.v1";
const UPDATE_OPERATION: &str = "customers.update.v1";
const CREATE_ADDRESS_OPERATION: &str = "customer_addresses.create.v1";
const DELETE_ADDRESS_OPERATION: &str = "customer_addresses.delete.v1";

type CustomerRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    OffsetDateTime,
    OffsetDateTime,
);
type AddressRow = (
    Uuid,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub struct PostgresCustomerRepository {
    runtime_pool: PgPool,
    identity_pool: PgPool,
}

impl PostgresCustomerRepository {
    pub fn new(runtime_pool: PgPool, identity_pool: PgPool) -> Self {
        Self {
            runtime_pool,
            identity_pool,
        }
    }

    async fn begin(
        &self,
        machine: &chaos_application::ports::MachineActor,
        user_id: UserId,
        shopper_id: Option<chaos_domain::sales::ShopperId>,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.runtime_pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(machine.store_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(user_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        if let Some(shopper_id) = shopper_id {
            sqlx::query("SELECT set_config('app.shopper_id', $1, true)")
                .bind(shopper_id.as_uuid().to_string())
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
        }
        Ok(transaction)
    }
}

#[async_trait]
impl CustomerRepository for PostgresCustomerRepository {
    async fn associate(
        &self,
        shopper: &ShopperActor,
        user_id: UserId,
        request: &IdempotencyRequest,
    ) -> Result<CustomerDetail, ApplicationError> {
        let email: String =
            sqlx::query_scalar("SELECT email::text FROM identity.users WHERE id = $1")
                .bind(user_id.as_uuid())
                .fetch_optional(&self.identity_pool)
                .await
                .map_err(database_error)?
                .ok_or(ApplicationError::Unauthorized)?;
        let machine = &shopper.machine;
        let channel_id = require_channel(machine)?;
        let mut transaction = self
            .begin(machine, user_id, Some(shopper.shopper_id))
            .await?;
        if let Some(id) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            ASSOCIATE_OPERATION,
            request,
        )
        .await?
        {
            let detail = load_customer_by_id(&mut transaction, machine, CustomerId::from_uuid(id))
                .await?
                .ok_or_else(corrupt_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        let customer_id = CustomerId::new();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO sales.customers \
             (id, store_id, user_id, email) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (store_id, user_id) DO UPDATE \
             SET email = EXCLUDED.email, updated_at = CURRENT_TIMESTAMP RETURNING id",
        )
        .bind(customer_id.as_uuid())
        .bind(machine.store_id.as_uuid())
        .bind(user_id.as_uuid())
        .bind(&email)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_customer_write_error)?;
        let customer_id = CustomerId::from_uuid(id);
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(shopper.shopper_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT customer_id FROM sales.customer_shopper_links \
             WHERE store_id = $1 AND shopper_id = $2",
        )
        .bind(machine.store_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        if existing.is_some_and(|existing| existing != customer_id.as_uuid()) {
            return Err(ApplicationError::Conflict {
                code: "shopper_already_associated",
                message: "the shopper credential is already associated with another Customer",
            });
        }
        if existing.is_none() {
            sqlx::query(
                "INSERT INTO sales.customer_shopper_links \
                 (store_id, customer_id, shopper_id, sales_channel_id) \
                 VALUES ($1,$2,$3,$4)",
            )
            .bind(machine.store_id.as_uuid())
            .bind(customer_id.as_uuid())
            .bind(shopper.shopper_id.as_uuid())
            .bind(channel_id.as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            ASSOCIATE_OPERATION,
            request,
            customer_id.as_uuid(),
        )
        .await?;
        let detail = load_customer_by_id(&mut transaction, machine, customer_id)
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get(&self, actor: &CustomerActor) -> Result<Option<CustomerDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor.machine, actor.user_id, None).await?;
        let value = load_customer_by_user(&mut transaction, actor).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn update_phone(
        &self,
        actor: &CustomerActor,
        phone: Option<&str>,
        request: &IdempotencyRequest,
    ) -> Result<CustomerDetail, ApplicationError> {
        let mut transaction = self.begin(&actor.machine, actor.user_id, None).await?;
        if let Some(id) = reserve(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            UPDATE_OPERATION,
            request,
        )
        .await?
        {
            let detail =
                load_customer_by_id(&mut transaction, &actor.machine, CustomerId::from_uuid(id))
                    .await?
                    .ok_or_else(customer_not_found)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        let mut detail = load_customer_by_user(&mut transaction, actor)
            .await?
            .ok_or_else(customer_not_found)?;
        detail.customer.update_phone(phone.map(Into::into))?;
        sqlx::query(
            "UPDATE sales.customers SET phone = $3, updated_at = CURRENT_TIMESTAMP \
            WHERE store_id = $1 AND id = $2",
        )
        .bind(actor.machine.store_id.as_uuid())
        .bind(detail.customer.id().as_uuid())
        .bind(detail.customer.contact().phone())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        complete(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            UPDATE_OPERATION,
            request,
            detail.customer.id().as_uuid(),
        )
        .await?;
        let detail = load_customer_by_id(&mut transaction, &actor.machine, detail.customer.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_address(
        &self,
        actor: &CustomerActor,
        address: &CustomerAddress,
        request: &IdempotencyRequest,
    ) -> Result<CustomerAddressDetail, ApplicationError> {
        let mut transaction = self.begin(&actor.machine, actor.user_id, None).await?;
        if let Some(id) = reserve(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            CREATE_ADDRESS_OPERATION,
            request,
        )
        .await?
        {
            let value = load_address(
                &mut transaction,
                &actor.machine,
                CustomerAddressId::from_uuid(id),
            )
            .await?
            .ok_or_else(address_not_found)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(value);
        }
        let customer = load_customer_by_user(&mut transaction, actor)
            .await?
            .ok_or_else(customer_not_found)?;
        let postal = address.address();
        sqlx::query(
            "INSERT INTO sales.customer_addresses \
             (id, store_id, customer_id, label, full_name, company, \
              address_line1, address_line2, locality, administrative_area, postal_code, country_code) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        ).bind(address.id().as_uuid())
            .bind(actor.machine.store_id.as_uuid()).bind(customer.customer.id().as_uuid())
            .bind(address.label()).bind(postal.full_name()).bind(postal.company())
            .bind(postal.address_line1()).bind(postal.address_line2()).bind(postal.locality())
            .bind(postal.administrative_area()).bind(postal.postal_code()).bind(postal.country_code())
            .execute(&mut *transaction).await.map_err(map_address_write_error)?;
        complete(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            CREATE_ADDRESS_OPERATION,
            request,
            address.id().as_uuid(),
        )
        .await?;
        let value = load_address(&mut transaction, &actor.machine, address.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn delete_address(
        &self,
        actor: &CustomerActor,
        address_id: CustomerAddressId,
        request: &IdempotencyRequest,
    ) -> Result<CustomerId, ApplicationError> {
        let mut transaction = self.begin(&actor.machine, actor.user_id, None).await?;
        if let Some(id) = reserve(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            DELETE_ADDRESS_OPERATION,
            request,
        )
        .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(CustomerId::from_uuid(id));
        }
        let customer = load_customer_by_user(&mut transaction, actor)
            .await?
            .ok_or_else(customer_not_found)?;
        let result = sqlx::query(
            "DELETE FROM sales.customer_addresses WHERE store_id = $1 \
            AND customer_id = $2 AND id = $3",
        )
        .bind(actor.machine.store_id.as_uuid())
        .bind(customer.customer.id().as_uuid())
        .bind(address_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(address_not_found());
        }
        complete(
            &mut transaction,
            &IdempotencyScope::User(actor.user_id.as_uuid()),
            DELETE_ADDRESS_OPERATION,
            request,
            customer.customer.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(customer.customer.id())
    }

    async fn list_orders(
        &self,
        actor: &CustomerActor,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<CustomerOrderPage, ApplicationError> {
        let channel_id = require_channel(&actor.machine)?;
        let mut transaction = self.begin(&actor.machine, actor.user_id, None).await?;
        let customer = load_customer_by_user(&mut transaction, actor)
            .await?
            .ok_or_else(customer_not_found)?;
        let ids = sqlx::query_as::<_, (Uuid, OffsetDateTime)>(
            "SELECT DISTINCT sales_order.id, sales_order.created_at FROM sales.orders AS sales_order \
             JOIN sales.customer_shopper_links AS link \
              ON link.store_id = sales_order.store_id AND link.shopper_id = sales_order.shopper_id \
             WHERE sales_order.store_id = $1 \
               AND sales_order.sales_channel_id = $2 AND link.customer_id = $3 \
               AND ($4::uuid IS NULL OR sales_order.id < $4) \
             ORDER BY sales_order.id DESC LIMIT $5",
        ).bind(actor.machine.store_id.as_uuid())
            .bind(channel_id.as_uuid()).bind(customer.customer.id().as_uuid()).bind(after)
            .bind(i64::from(limit) + 1).fetch_all(&mut *transaction).await.map_err(database_error)?;
        let has_more = ids.len() > usize::from(limit);
        let mut items = Vec::with_capacity(ids.len().min(usize::from(limit)));
        for (id, _) in ids.into_iter().take(usize::from(limit)) {
            items.push(
                load_order(
                    &mut transaction,
                    &actor.machine,
                    chaos_domain::sales::OrderId::from_uuid(id),
                )
                .await?
                .ok_or_else(corrupt_state)?,
            );
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(CustomerOrderPage { items, has_more })
    }
}

async fn load_customer_by_user(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &CustomerActor,
) -> Result<Option<CustomerDetail>, ApplicationError> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM sales.customers WHERE store_id = $1 AND user_id = $2")
            .bind(actor.machine.store_id.as_uuid())
            .bind(actor.user_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(database_error)?;
    match id {
        Some(id) => {
            load_customer_by_id(transaction, &actor.machine, CustomerId::from_uuid(id)).await
        }
        None => Ok(None),
    }
}

async fn load_customer_by_id(
    transaction: &mut Transaction<'static, Postgres>,
    machine: &chaos_application::ports::MachineActor,
    id: CustomerId,
) -> Result<Option<CustomerDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CustomerRow>(
        "SELECT id, user_id, email::text, phone, created_at, updated_at \
        FROM sales.customers WHERE store_id = $1 AND id = $2",
    )
    .bind(machine.store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let addresses = sqlx::query_as::<_, AddressRow>("SELECT id, label, full_name, company, address_line1, \
        address_line2, locality, administrative_area, postal_code, country_code::text, created_at, updated_at \
        FROM sales.customer_addresses WHERE store_id = $1 AND customer_id = $2 \
        ORDER BY created_at, id")
        .bind(machine.store_id.as_uuid()).bind(row.0)
        .fetch_all(&mut **transaction).await.map_err(database_error)?;
    Ok(Some(CustomerDetail {
        customer: Customer::rehydrate(
            CustomerId::from_uuid(row.0),
            UserId::from_uuid(row.1),
            row.2,
            row.3,
        )?,
        addresses: addresses
            .into_iter()
            .map(address_detail)
            .collect::<Result<Vec<_>, _>>()?,
        created_at: row.4,
        updated_at: row.5,
    }))
}

async fn load_address(
    transaction: &mut Transaction<'static, Postgres>,
    machine: &chaos_application::ports::MachineActor,
    id: CustomerAddressId,
) -> Result<Option<CustomerAddressDetail>, ApplicationError> {
    sqlx::query_as::<_, AddressRow>(
        "SELECT id, label, full_name, company, address_line1, address_line2, \
        locality, administrative_area, postal_code, country_code::text, created_at, updated_at \
        FROM sales.customer_addresses WHERE store_id = $1 AND id = $2",
    )
    .bind(machine.store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(address_detail)
    .transpose()
}

fn address_detail(row: AddressRow) -> Result<CustomerAddressDetail, ApplicationError> {
    let address = PostalAddress::new(row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9)?;
    Ok(CustomerAddressDetail {
        address: CustomerAddress::rehydrate(CustomerAddressId::from_uuid(row.0), row.1, address)?,
        created_at: row.10,
        updated_at: row.11,
    })
}

fn require_channel(
    machine: &chaos_application::ports::MachineActor,
) -> Result<SalesChannelId, ApplicationError> {
    machine.sales_channel_id.ok_or_else(|| {
        ApplicationError::Unexpected(anyhow::anyhow!(
            "Customer operation requires a Sales Channel"
        ))
    })
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(body) = idempotency::reserve(transaction, scope, operation, request).await? else {
        return Ok(None);
    };
    body.pointer("/data/id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(Some)
        .ok_or_else(corrupt_state)
}
async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: Uuid,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        scope,
        operation,
        request,
        200,
        json!({"data":{"id":id}}),
    )
    .await
}

fn map_customer_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("customers_store_id_email_key")
    {
        return ApplicationError::Conflict {
            code: "customer_email_taken",
            message: "the email is already associated with another Customer in this Store",
        };
    }
    database_error(error)
}
fn map_address_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("customer_addresses_customer_label_key")
    {
        return ApplicationError::Conflict {
            code: "customer_address_label_taken",
            message: "the address label is already in use for this Customer",
        };
    }
    database_error(error)
}
fn customer_not_found() -> ApplicationError {
    ApplicationError::NotFound {
        resource: "customer",
        id: "current".into(),
    }
}
fn address_not_found() -> ApplicationError {
    ApplicationError::NotFound {
        resource: "customer_address",
        id: "current".into(),
    }
}
fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains invalid Customer state"))
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
