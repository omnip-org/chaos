use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, PreparedShippingLabelCancellation,
        PreparedShippingLabelPurchase, PreparedShippingQuote, PurchasedShippingLabel,
        ShippingAddress, ShippingCancellationJob, ShippingCancellationStatus, ShippingLabelDetail,
        ShippingOperationRepository, ShippingParcel, ShippingProviderAccountConfiguration,
        ShippingProviderAccountDetail, ShippingProviderAccountRepository, ShippingQuoteCommand,
        ShippingRateQuote, ShippingRateQuoteDetail, ShippingServiceDetail,
        ShippingServiceRepository, ShippingTrackingJob, ShippingTrackingQueue,
        ShippingTrackingSnapshot,
    },
};
use chaos_domain::{
    CurrencyCode,
    fulfillment::{
        FulfillmentId, ShippingLabelId, ShippingProviderAccount, ShippingProviderAccountId,
        ShippingQuoteRequestId, ShippingRateQuoteId, ShippingSecretReference, ShippingService,
        ShippingServiceId, ShippingServiceStatus,
    },
    pricing::Money,
    store::StoreId,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

const CREATE_OPERATION: &str = "shipping_services.create.v1";
const ACTIVATE_OPERATION: &str = "shipping_services.activate.v1";
const ARCHIVE_OPERATION: &str = "shipping_services.archive.v1";
const CREATE_PROVIDER_ACCOUNT_OPERATION: &str = "shipping_provider_accounts.create.v1";
const UPDATE_PROVIDER_ACCOUNT_OPERATION: &str = "shipping_provider_accounts.update.v1";

type ServiceRow = (
    Uuid,
    String,
    String,
    i64,
    String,
    i16,
    i16,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(sqlx::FromRow)]
struct ProviderAccountRow {
    id: Uuid,
    provider: String,
    display_name: String,
    enabled: bool,
    credentials_configured: bool,
    origin_name: String,
    origin_company: Option<String>,
    origin_address_line_1: String,
    origin_address_line_2: Option<String>,
    origin_city: String,
    origin_region: Option<String>,
    origin_postal_code: String,
    origin_country_code: String,
    origin_phone: Option<String>,
    origin_email: Option<String>,
    credential_rotation_expires_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct QuoteContextRow {
    provider: String,
    credential_secret_reference: String,
    origin_name: String,
    origin_company: Option<String>,
    origin_address_line_1: String,
    origin_address_line_2: Option<String>,
    origin_city: String,
    origin_region: Option<String>,
    origin_postal_code: String,
    origin_country_code: String,
    origin_phone: Option<String>,
    origin_email: Option<String>,
    destination_name: String,
    destination_company: Option<String>,
    destination_address_line_1: String,
    destination_address_line_2: Option<String>,
    destination_city: String,
    destination_region: Option<String>,
    destination_postal_code: String,
    destination_country_code: String,
    destination_phone: Option<String>,
    destination_email: String,
}

#[derive(sqlx::FromRow)]
struct RateRow {
    id: Uuid,
    quote_request_id: Uuid,
    provider_shipment_reference: String,
    provider_rate_reference: String,
    carrier: String,
    service: String,
    amount_minor: i64,
    currency: String,
    estimated_delivery_days: Option<i16>,
    guaranteed: bool,
    expires_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct LabelRow {
    id: Uuid,
    fulfillment_id: Uuid,
    rate_quote_id: Uuid,
    provider: String,
    provider_shipment_reference: String,
    carrier: String,
    tracking_number: String,
    provider_tracker_reference: Option<String>,
    label_url: String,
    label_media_type: String,
    cancellation_status: Option<String>,
    tracking_status: Option<String>,
    tracking_status_detail: Option<String>,
    estimated_delivery_at: Option<OffsetDateTime>,
    tracking_observed_at: Option<OffsetDateTime>,
    purchased_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct TrackingClaimRow {
    label_id: Uuid,
    store_id: Uuid,
    fulfillment_id: Uuid,
    provider: String,
    provider_tracker_reference: String,
    credential_secret_reference: String,
    attempts: i32,
}

#[derive(sqlx::FromRow)]
struct CancellationClaimRow {
    label_id: Uuid,
    store_id: Uuid,
    fulfillment_id: Uuid,
    provider: String,
    provider_shipment_reference: String,
    credential_secret_reference: String,
    attempts: i32,
}

#[derive(Clone)]
pub struct PostgresShippingServiceRepository {
    pool: PgPool,
}

impl PostgresShippingServiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl ShippingServiceRepository for PostgresShippingServiceRepository {
    async fn create_shipping_service(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        service: &ShippingService,
        request: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut transaction, &actor, CREATE_OPERATION, request).await? {
            let detail = load(
                &mut transaction,
                &actor,
                store_id,
                ShippingServiceId::from_uuid(id),
            )
            .await?
            .ok_or_else(|| service_not_found(ShippingServiceId::from_uuid(id)))?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        require_store_currency(
            &mut transaction,
            &actor,
            store_id,
            service.rate().currency(),
        )
        .await?;
        sqlx::query(
            "INSERT INTO commerce.shipping_services \
             (id, store_id, code, name, amount_minor, currency, \
              estimated_min_days, estimated_max_days, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::commerce.shipping_service_status)",
        )
        .bind(service.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(service.code())
        .bind(service.name())
        .bind(service.rate().amount_minor())
        .bind(service.rate().currency().as_str())
        .bind(i16::try_from(service.estimated_min_days()).map_err(conversion_error)?)
        .bind(i16::try_from(service.estimated_max_days()).map_err(conversion_error)?)
        .bind(service.status().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_create_error)?;
        for country in service.destination_countries() {
            sqlx::query(
                "INSERT INTO commerce.shipping_service_regions \
                 (store_id, shipping_service_id, country_code) \
                 VALUES ($1, $2, $3)",
            )
            .bind(store_id.as_uuid())
            .bind(service.id().as_uuid())
            .bind(country)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        complete(
            &mut transaction,
            &actor,
            CREATE_OPERATION,
            request,
            service.id(),
        )
        .await?;
        let detail = load(&mut transaction, &actor, store_id, service.id())
            .await?
            .ok_or_else(|| service_not_found(service.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn list_shipping_services(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingServiceDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        require_store(&mut transaction, &actor, store_id).await?;
        let rows = sqlx::query_as::<_, ServiceRow>(
            "SELECT id, code, name, amount_minor, currency::text, estimated_min_days, \
                    estimated_max_days, status::text, created_at, updated_at \
             FROM commerce.shipping_services \
             WHERE store_id = $1 ORDER BY created_at, id",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let mut details = Vec::with_capacity(rows.len());
        for row in rows {
            details.push(detail(&mut transaction, &actor, store_id, row).await?);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(details)
    }

    async fn change_shipping_service_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        service_id: ShippingServiceId,
        status: ShippingServiceStatus,
        request: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        let operation = match status {
            ShippingServiceStatus::Active => ACTIVATE_OPERATION,
            ShippingServiceStatus::Archived => ARCHIVE_OPERATION,
        };
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, operation, request)
            .await?
            .is_none()
        {
            let result = sqlx::query(
                "UPDATE commerce.shipping_services SET status = $1::commerce.shipping_service_status, \
                        updated_at = CURRENT_TIMESTAMP \
                 WHERE store_id = $2 AND id = $3",
            )
            .bind(status.as_str())
            .bind(store_id.as_uuid())
            .bind(service_id.as_uuid())
            .execute(&mut *transaction).await.map_err(database_error)?;
            if result.rows_affected() != 1 {
                return Err(service_not_found(service_id));
            }
            complete(&mut transaction, &actor, operation, request, service_id).await?;
        }
        let detail = load(&mut transaction, &actor, store_id, service_id)
            .await?
            .ok_or_else(|| service_not_found(service_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

#[async_trait]
impl ShippingProviderAccountRepository for PostgresShippingServiceRepository {
    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        require_store(&mut transaction, &actor, store_id).await?;
        let rows = sqlx::query_as::<_, ProviderAccountRow>(
            "SELECT id, provider, display_name, enabled, \
                    credential_secret_reference IS NOT NULL AS credentials_configured, \
                    origin_name, origin_company, origin_address_line_1, origin_address_line_2, \
                    origin_city, origin_region, origin_postal_code, origin_country_code::text, \
                    origin_phone, origin_email, credential_rotation_expires_at, created_at, updated_at \
             FROM commerce.shipping_provider_accounts \
             WHERE store_id = $1 ORDER BY created_at, id",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let values = rows
            .into_iter()
            .map(provider_account_detail)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(values)
    }

    async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        id: ShippingProviderAccountId,
    ) -> Result<Option<ShippingProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let value = load_provider_account(&mut transaction, store_id, id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn create(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if let Some(id) = reserve(
            &mut transaction,
            &actor,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            let value = load_provider_account(
                &mut transaction,
                store_id,
                ShippingProviderAccountId::from_uuid(id),
            )
            .await?
            .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(value);
        }
        let origin = &configuration.origin;
        sqlx::query(
            "INSERT INTO commerce.shipping_provider_accounts \
             (id, store_id, provider, display_name, credential_secret_reference, \
              origin_name, origin_company, origin_address_line_1, origin_address_line_2, origin_city, \
              origin_region, origin_postal_code, origin_country_code, origin_phone, origin_email, \
              enabled, created_by_user_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
        )
        .bind(account.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(account.provider())
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(&origin.name)
        .bind(&origin.company)
        .bind(&origin.address_line_1)
        .bind(&origin.address_line_2)
        .bind(&origin.city)
        .bind(&origin.region)
        .bind(&origin.postal_code)
        .bind(&origin.country_code)
        .bind(&origin.phone)
        .bind(&origin.email)
        .bind(account.enabled())
        .bind(audit_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        complete_provider_account(
            &mut transaction,
            &actor,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn update(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if let Some(id) = reserve(
            &mut transaction,
            &actor,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            let value = load_provider_account(
                &mut transaction,
                store_id,
                ShippingProviderAccountId::from_uuid(id),
            )
            .await?
            .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(value);
        }
        let origin = &configuration.origin;
        let result = sqlx::query(
            "UPDATE commerce.shipping_provider_accounts SET display_name = $1, \
                    previous_credential_secret_reference = CASE \
                        WHEN credential_secret_reference IS DISTINCT FROM $2 \
                        THEN credential_secret_reference ELSE previous_credential_secret_reference END, \
                    credential_rotation_expires_at = CASE \
                        WHEN credential_secret_reference IS DISTINCT FROM $3 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE credential_rotation_expires_at END, \
                    credential_secret_reference = $4, origin_name = $5, origin_company = $6, \
                    origin_address_line_1 = $7, origin_address_line_2 = $8, origin_city = $9, \
                    origin_region = $10, origin_postal_code = $11, origin_country_code = $12, \
                    origin_phone = $13, origin_email = $14, enabled = $15, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $16 AND id = $17",
        )
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(&origin.name)
        .bind(&origin.company)
        .bind(&origin.address_line_1)
        .bind(&origin.address_line_2)
        .bind(&origin.city)
        .bind(&origin.region)
        .bind(&origin.postal_code)
        .bind(&origin.country_code)
        .bind(&origin.phone)
        .bind(&origin.email)
        .bind(account.enabled())
        .bind(store_id.as_uuid())
        .bind(account.id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(provider_account_not_found(account.id()));
        }
        complete_provider_account(
            &mut transaction,
            &actor,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, store_id, account.id())
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}

#[async_trait]
impl ShippingOperationRepository for PostgresShippingServiceRepository {
    async fn prepare_quote(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        provider_account_id: ShippingProviderAccountId,
        parcel: ShippingParcel,
        request: &IdempotencyRequest,
    ) -> Result<PreparedShippingQuote, ApplicationError> {
        validate_parcel(parcel)?;
        let mut transaction = self.begin(&actor).await?;
        if let Some(row) = sqlx::query_as::<_, (Uuid, Vec<u8>, String)>(
            "SELECT id, request_fingerprint, state FROM commerce.shipping_quote_requests \
             WHERE store_id = $1 AND idempotency_key = $2 \
             FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(&request.key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        {
            if row.1.as_slice() != request.request_fingerprint {
                return Err(idempotency_reused());
            }
            let id = ShippingQuoteRequestId::from_uuid(row.0);
            if row.2 == "completed" {
                let rates = load_rates(&mut transaction, store_id, id).await?;
                transaction.commit().await.map_err(database_error)?;
                return Ok(PreparedShippingQuote::Completed(rates));
            }
        }
        let context = load_quote_context(
            &mut transaction,
            store_id,
            fulfillment_id,
            provider_account_id,
        )
        .await?;
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO commerce.shipping_quote_requests \
             (id, store_id, fulfillment_id, provider_account_id, \
              idempotency_key, request_fingerprint, length_millimetres, width_millimetres, \
              height_millimetres, weight_grams) \
             VALUES (uuidv7(),$1,$2,$3,$4,$5,$6,$7,$8,$9) \
             ON CONFLICT (store_id, idempotency_key) DO NOTHING RETURNING id",
        )
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .bind(provider_account_id.as_uuid())
        .bind(&request.key)
        .bind(request.request_fingerprint.as_slice())
        .bind(i32::try_from(parcel.length_millimetres).map_err(conversion_error)?)
        .bind(i32::try_from(parcel.width_millimetres).map_err(conversion_error)?)
        .bind(i32::try_from(parcel.height_millimetres).map_err(conversion_error)?)
        .bind(i32::try_from(parcel.weight_grams).map_err(conversion_error)?)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let quote_request_id = match inserted {
            Some(id) => ShippingQuoteRequestId::from_uuid(id),
            None => {
                let row = sqlx::query_as::<_, (Uuid, Vec<u8>, String)>(
                    "SELECT id, request_fingerprint, state FROM commerce.shipping_quote_requests \
                 WHERE store_id = $1 AND idempotency_key = $2 \
                 FOR UPDATE",
                )
                .bind(store_id.as_uuid())
                .bind(&request.key)
                .fetch_one(&mut *transaction)
                .await
                .map_err(database_error)?;
                if row.1.as_slice() != request.request_fingerprint {
                    return Err(idempotency_reused());
                }
                let id = ShippingQuoteRequestId::from_uuid(row.0);
                if row.2 == "completed" {
                    let rates = load_rates(&mut transaction, store_id, id).await?;
                    transaction.commit().await.map_err(database_error)?;
                    return Ok(PreparedShippingQuote::Completed(rates));
                }
                id
            }
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(PreparedShippingQuote::Pending {
            quote_request_id,
            provider: context.provider.clone(),
            command: Box::new(ShippingQuoteCommand {
                from: origin_address(&context),
                to: destination_address(&context),
                parcel,
                idempotency_key: format!("shipping-quote-{}", quote_request_id.as_uuid()),
                credential_secret_reference: ShippingSecretReference::new(
                    context.credential_secret_reference,
                )?,
            }),
        })
    }

    async fn complete_quote(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        quote_request_id: ShippingQuoteRequestId,
        rates: Vec<ShippingRateQuote>,
        completed_at: OffsetDateTime,
    ) -> Result<Vec<ShippingRateQuoteDetail>, ApplicationError> {
        if rates.is_empty() {
            return Err(ApplicationError::Conflict {
                code: "shipping_rates_unavailable",
                message: "the Shipping Provider returned no available rates",
            });
        }
        let mut transaction = self.begin(&actor).await?;
        let state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM commerce.shipping_quote_requests \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(quote_request_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| quote_not_found(quote_request_id))?;
        if state == "completed" {
            let rates = load_rates(&mut transaction, store_id, quote_request_id).await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(rates);
        }
        let expires_at = completed_at + time::Duration::hours(24);
        for rate in rates {
            sqlx::query(
                "INSERT INTO commerce.shipping_rate_quotes \
                 (id, store_id, quote_request_id, \
                  provider_shipment_reference, provider_rate_reference, carrier, service, \
                  amount_minor, currency, estimated_delivery_days, guaranteed, expires_at) \
                 VALUES (uuidv7(),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(store_id.as_uuid())
            .bind(quote_request_id.as_uuid())
            .bind(rate.provider_shipment_reference)
            .bind(rate.provider_rate_reference)
            .bind(rate.carrier)
            .bind(rate.service)
            .bind(rate.amount_minor)
            .bind(rate.currency.as_str())
            .bind(
                rate.estimated_delivery_days
                    .map(i16::try_from)
                    .transpose()
                    .map_err(conversion_error)?,
            )
            .bind(rate.guaranteed)
            .bind(expires_at)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        sqlx::query(
            "UPDATE commerce.shipping_quote_requests \
             SET state = 'completed', expires_at = $1, updated_at = $2 \
             WHERE store_id = $3 AND id = $4",
        )
        .bind(expires_at)
        .bind(completed_at)
        .bind(store_id.as_uuid())
        .bind(quote_request_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let rates = load_rates(&mut transaction, store_id, quote_request_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(rates)
    }

    async fn prepare_label_purchase(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        rate_quote_id: ShippingRateQuoteId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<PreparedShippingLabelPurchase, ApplicationError> {
        prepare_label_purchase(
            self,
            actor,
            store_id,
            fulfillment_id,
            rate_quote_id,
            request,
            now,
        )
        .await
    }

    async fn complete_label_purchase(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        label_id: ShippingLabelId,
        label: PurchasedShippingLabel,
        completed_at: OffsetDateTime,
    ) -> Result<ShippingLabelDetail, ApplicationError> {
        complete_label_purchase(self, actor, store_id, label_id, label, completed_at).await
    }

    async fn prepare_label_cancellation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<PreparedShippingLabelCancellation, ApplicationError> {
        prepare_label_cancellation(self, actor, store_id, fulfillment_id, request, now).await
    }

    async fn complete_label_cancellation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        label_id: ShippingLabelId,
        status: ShippingCancellationStatus,
        completed_at: OffsetDateTime,
    ) -> Result<ShippingLabelDetail, ApplicationError> {
        complete_label_cancellation(self, actor, store_id, label_id, status, completed_at).await
    }
}

async fn prepare_label_purchase(
    repository: &PostgresShippingServiceRepository,
    actor: AdminActor,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
    rate_quote_id: ShippingRateQuoteId,
    request: &IdempotencyRequest,
    now: OffsetDateTime,
) -> Result<PreparedShippingLabelPurchase, ApplicationError> {
    let mut transaction = repository.begin(&actor).await?;
    if let Some(row) = sqlx::query_as::<_, (Uuid, Vec<u8>, String)>(
        "SELECT id, purchase_request_fingerprint, purchase_state \
         FROM commerce.shipping_labels \
         WHERE store_id = $1 AND fulfillment_id = $2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    {
        if row.1.as_slice() != request.request_fingerprint {
            return Err(ApplicationError::Conflict {
                code: "shipping_label_already_requested",
                message: "a Shipping Label was already requested for this Fulfillment",
            });
        }
        let label_id = ShippingLabelId::from_uuid(row.0);
        if row.2 == "purchased" {
            let detail = load_label(&mut transaction, store_id, label_id)
                .await?
                .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(PreparedShippingLabelPurchase::Completed(Box::new(detail)));
        }
        let pending = load_pending_label(&mut transaction, store_id, label_id).await?;
        transaction.commit().await.map_err(database_error)?;
        return Ok(pending);
    }
    let label_id = ShippingLabelId::new();
    let inserted = sqlx::query(
        "INSERT INTO commerce.shipping_labels \
         (id, store_id, fulfillment_id, provider_account_id, rate_quote_id, \
          purchase_idempotency_key, purchase_request_fingerprint, \
          provider_shipment_reference, provider_rate_reference) \
         SELECT $1, $2, $3, request.provider_account_id, rate.id, $4, $5, \
                rate.provider_shipment_reference, rate.provider_rate_reference \
         FROM commerce.shipping_rate_quotes AS rate \
         INNER JOIN commerce.shipping_quote_requests AS request \
            ON request.store_id = rate.store_id AND request.id = rate.quote_request_id \
         INNER JOIN commerce.fulfillments AS fulfillment \
            ON commerce.store_id = rate.store_id AND commerce.id = $6 \
         INNER JOIN commerce.shipping_provider_accounts AS account \
            ON account.store_id = request.store_id AND account.id = request.provider_account_id \
         WHERE rate.store_id = $7 AND rate.id = $8 \
           AND request.fulfillment_id = $9 AND rate.expires_at > $10 \
           AND commerce.status = 'pending' AND account.enabled",
    )
    .bind(label_id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .bind(&request.key)
    .bind(request.request_fingerprint.as_slice())
    .bind(fulfillment_id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(rate_quote_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(map_label_write_error)?;
    if inserted.rows_affected() != 1 {
        return Err(ApplicationError::Conflict {
            code: "shipping_rate_not_purchasable",
            message: "the Shipping Rate is expired, disabled, or not valid for this Fulfillment",
        });
    }
    let pending = load_pending_label(&mut transaction, store_id, label_id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(pending)
}

async fn load_pending_label(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    label_id: ShippingLabelId,
) -> Result<PreparedShippingLabelPurchase, ApplicationError> {
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT account.provider, label.provider_shipment_reference, \
                label.provider_rate_reference, account.credential_secret_reference \
         FROM commerce.shipping_labels AS label \
         INNER JOIN commerce.shipping_provider_accounts AS account \
            ON account.store_id = label.store_id AND account.id = label.provider_account_id \
         WHERE label.store_id = $1 AND label.id = $2 \
        ",
    )
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| label_not_found(label_id))?;
    Ok(PreparedShippingLabelPurchase::Pending {
        label_id,
        provider: row.0,
        provider_shipment_reference: row.1,
        provider_rate_reference: row.2,
        credential_secret_reference: ShippingSecretReference::new(row.3)?,
        operation_key: format!("shipping-label-{}", label_id.as_uuid()),
    })
}

async fn complete_label_purchase(
    repository: &PostgresShippingServiceRepository,
    actor: AdminActor,
    store_id: StoreId,
    label_id: ShippingLabelId,
    label: PurchasedShippingLabel,
    completed_at: OffsetDateTime,
) -> Result<ShippingLabelDetail, ApplicationError> {
    let mut transaction = repository.begin(&actor).await?;
    let row = sqlx::query_as::<_, (String, Uuid, String)>(
        "SELECT purchase_state, fulfillment_id, provider_shipment_reference \
         FROM commerce.shipping_labels \
         WHERE store_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| label_not_found(label_id))?;
    if row.0 == "purchased" {
        let detail = load_label(&mut transaction, store_id, label_id)
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        return Ok(detail);
    }
    if row.2 != label.provider_shipment_reference {
        return Err(ApplicationError::Conflict {
            code: "shipping_label_provider_mismatch",
            message: "the purchased label does not match the quoted Provider Shipment",
        });
    }
    sqlx::query(
        "UPDATE commerce.shipping_labels SET purchase_state = 'purchased', carrier = $3, \
                tracking_number = $4, provider_tracker_reference = $5, label_url = $6, \
                label_media_type = $7, purchased_at = $8, \
                next_tracking_refresh_at = CASE WHEN $5::text IS NULL THEN NULL ELSE $8 END, \
                updated_at = $8 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .bind(&label.carrier)
    .bind(&label.tracking_number)
    .bind(&label.provider_tracker_reference)
    .bind(&label.label_url)
    .bind(&label.label_media_type)
    .bind(completed_at)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    let order_id = sqlx::query_scalar::<_, Uuid>(
        "UPDATE commerce.fulfillments SET status = 'shipped', carrier = $1, \
                tracking_number = $2, shipped_at = $3, updated_at = $3 \
         WHERE store_id = $4 AND id = $5 AND status = 'pending' \
         RETURNING order_id",
    )
    .bind(&label.carrier)
    .bind(&label.tracking_number)
    .bind(completed_at)
    .bind(store_id.as_uuid())
    .bind(row.1)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or(ApplicationError::Conflict {
        code: "fulfillment_not_pending",
        message: "the Fulfillment can no longer accept a Shipping Label",
    })?;
    sqlx::query(
        "INSERT INTO integration.event_outbox \
         (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES (uuidv7(), $1, 'fulfillment', $2, 'fulfillment.shipped', $3)",
    )
    .bind(store_id.as_uuid())
    .bind(row.1)
    .bind(json!({
        "fulfillment_id": row.1,
        "order_id": order_id,
        "status": "shipped",
    }))
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    let detail = load_label(&mut transaction, store_id, label_id)
        .await?
        .ok_or_else(corrupt_provider_state)?;
    transaction.commit().await.map_err(database_error)?;
    Ok(detail)
}

async fn prepare_label_cancellation(
    repository: &PostgresShippingServiceRepository,
    actor: AdminActor,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
    request: &IdempotencyRequest,
    now: OffsetDateTime,
) -> Result<PreparedShippingLabelCancellation, ApplicationError> {
    let mut transaction = repository.begin(&actor).await?;
    let row = sqlx::query_as::<_, (Uuid, Option<String>, Option<Vec<u8>>, Option<String>)>(
        "SELECT id, cancellation_idempotency_key, cancellation_request_fingerprint, \
                cancellation_status \
         FROM commerce.shipping_labels \
         WHERE store_id = $1 AND fulfillment_id = $2 \
           AND purchase_state = 'purchased' FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::NotFound {
        resource: "shipping_label",
        id: fulfillment_id.as_uuid().to_string(),
    })?;
    let label_id = ShippingLabelId::from_uuid(row.0);
    if let Some(key) = row.1 {
        if key != request.key || row.2.as_deref() != Some(request.request_fingerprint.as_slice()) {
            return Err(ApplicationError::Conflict {
                code: "shipping_label_cancellation_already_requested",
                message: "cancellation was already requested for this Shipping Label",
            });
        }
        if row.3.as_deref().is_some_and(|status| status != "submitted") {
            let detail = load_label(&mut transaction, store_id, label_id)
                .await?
                .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(PreparedShippingLabelCancellation::Completed(Box::new(
                detail,
            )));
        }
    } else {
        sqlx::query(
            "UPDATE commerce.shipping_labels \
             SET cancellation_idempotency_key = $1, cancellation_request_fingerprint = $2, \
                 cancellation_status = 'submitted', cancellation_requested_at = $3, updated_at = $4 \
             WHERE store_id = $5 AND id = $6",
        )
        .bind(&request.key)
        .bind(request.request_fingerprint.as_slice())
        .bind(now)
        .bind(now)
        .bind(store_id.as_uuid())
        .bind(label_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(map_label_write_error)?;
    }
    let pending = load_pending_cancellation(&mut transaction, store_id, label_id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(pending)
}

async fn load_pending_cancellation(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    label_id: ShippingLabelId,
) -> Result<PreparedShippingLabelCancellation, ApplicationError> {
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT account.provider, label.provider_shipment_reference, \
                account.credential_secret_reference \
         FROM commerce.shipping_labels AS label \
         INNER JOIN commerce.shipping_provider_accounts AS account \
            ON account.store_id = label.store_id AND account.id = label.provider_account_id \
         WHERE label.store_id = $1 AND label.id = $2 \
        ",
    )
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| label_not_found(label_id))?;
    Ok(PreparedShippingLabelCancellation::Pending {
        label_id,
        provider: row.0,
        provider_shipment_reference: row.1,
        credential_secret_reference: ShippingSecretReference::new(row.2)?,
    })
}

async fn complete_label_cancellation(
    repository: &PostgresShippingServiceRepository,
    actor: AdminActor,
    store_id: StoreId,
    label_id: ShippingLabelId,
    status: ShippingCancellationStatus,
    completed_at: OffsetDateTime,
) -> Result<ShippingLabelDetail, ApplicationError> {
    let mut transaction = repository.begin(&actor).await?;
    let result = sqlx::query(
        "UPDATE commerce.shipping_labels SET cancellation_status = $1, \
                cancellation_reconcile_at = CASE WHEN $2 = 'submitted' \
                    THEN $3 + INTERVAL '15 minutes' ELSE NULL END, \
                cancellation_locked_by = NULL, cancellation_locked_at = NULL, \
                cancellation_attempts = 0, cancellation_last_error = NULL, updated_at = $4 \
         WHERE store_id = $5 AND id = $6 \
           AND cancellation_idempotency_key IS NOT NULL",
    )
    .bind(cancellation_status_text(status))
    .bind(cancellation_status_text(status))
    .bind(completed_at)
    .bind(completed_at)
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(label_not_found(label_id));
    }
    let detail = load_label(&mut transaction, store_id, label_id)
        .await?
        .ok_or_else(corrupt_provider_state)?;
    transaction.commit().await.map_err(database_error)?;
    Ok(detail)
}

async fn load_label(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    label_id: ShippingLabelId,
) -> Result<Option<ShippingLabelDetail>, ApplicationError> {
    sqlx::query_as::<_, LabelRow>(
        "SELECT label.id, label.fulfillment_id, label.rate_quote_id, account.provider, \
                label.provider_shipment_reference, label.carrier, label.tracking_number, \
                label.provider_tracker_reference, label.label_url, label.label_media_type, \
                label.cancellation_status, label.tracking_status, label.tracking_status_detail, \
                label.estimated_delivery_at, label.tracking_observed_at, label.purchased_at, \
                label.updated_at \
         FROM commerce.shipping_labels AS label \
         INNER JOIN commerce.shipping_provider_accounts AS account \
            ON account.store_id = label.store_id AND account.id = label.provider_account_id \
         WHERE label.store_id = $1 AND label.id = $2 \
           AND label.purchase_state = 'purchased'",
    )
    .bind(store_id.as_uuid())
    .bind(label_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(label_detail)
    .transpose()
}

fn label_detail(row: LabelRow) -> Result<ShippingLabelDetail, ApplicationError> {
    let tracking = match (row.tracking_status, row.tracking_observed_at) {
        (Some(status), Some(observed_at)) => Some(ShippingTrackingSnapshot {
            status: tracking_status(&status).ok_or_else(corrupt_provider_state)?,
            status_detail: row.tracking_status_detail,
            estimated_delivery_at: row.estimated_delivery_at,
            observed_at,
        }),
        (None, None) => None,
        _ => return Err(corrupt_provider_state()),
    };
    Ok(ShippingLabelDetail {
        id: ShippingLabelId::from_uuid(row.id),
        fulfillment_id: FulfillmentId::from_uuid(row.fulfillment_id),
        rate_quote_id: ShippingRateQuoteId::from_uuid(row.rate_quote_id),
        provider: row.provider,
        label: PurchasedShippingLabel {
            provider_shipment_reference: row.provider_shipment_reference,
            carrier: row.carrier,
            tracking_number: row.tracking_number,
            provider_tracker_reference: row.provider_tracker_reference,
            label_url: row.label_url,
            label_media_type: row.label_media_type,
        },
        cancellation_status: row
            .cancellation_status
            .map(|status| parse_cancellation_status(&status).ok_or_else(corrupt_provider_state))
            .transpose()?,
        tracking,
        purchased_at: row.purchased_at,
        updated_at: row.updated_at,
    })
}

#[async_trait]
impl ShippingTrackingQueue for PostgresShippingServiceRepository {
    async fn claim_tracking(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<ShippingTrackingJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, TrackingClaimRow>(
            "SELECT label_id, store_id, fulfillment_id, provider, \
                    provider_tracker_reference, credential_secret_reference, attempts \
             FROM commerce.claim_shipping_tracking($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(ShippingTrackingJob {
                    label_id: ShippingLabelId::from_uuid(row.label_id),
                    store_id: StoreId::from_uuid(row.store_id),
                    fulfillment_id: FulfillmentId::from_uuid(row.fulfillment_id),
                    provider: row.provider,
                    provider_tracker_reference: row.provider_tracker_reference,
                    credential_secret_reference: ShippingSecretReference::new(
                        row.credential_secret_reference,
                    )?,
                    attempts: u32::try_from(row.attempts).map_err(conversion_error)?,
                })
            })
            .collect()
    }

    async fn finish_tracking(
        &self,
        worker_id: Uuid,
        job: &ShippingTrackingJob,
        result: Result<ShippingTrackingSnapshot, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        match result {
            Ok(snapshot) => {
                let terminal = matches!(
                    snapshot.status,
                    chaos_application::ports::ProviderTrackingStatus::Delivered
                );
                let result = sqlx::query(
                    "UPDATE commerce.shipping_labels SET tracking_status = $1, \
                            tracking_status_detail = $2, estimated_delivery_at = $3, \
                            tracking_observed_at = $4, next_tracking_refresh_at = CASE \
                                WHEN $5 THEN NULL ELSE $6 + INTERVAL '15 minutes' END, \
                            tracking_locked_by = NULL, tracking_locked_at = NULL, \
                            tracking_attempts = 0, tracking_last_error = NULL, updated_at = $7 \
                     WHERE store_id = $8 AND id = $9 \
                       AND tracking_locked_by = $10",
                )
                .bind(tracking_status_text(snapshot.status))
                .bind(snapshot.status_detail)
                .bind(snapshot.estimated_delivery_at)
                .bind(snapshot.observed_at)
                .bind(terminal)
                .bind(now)
                .bind(now)
                .bind(job.store_id.as_uuid())
                .bind(job.label_id.as_uuid())
                .bind(worker_id)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                if result.rows_affected() != 1 {
                    return Err(stale_tracking_lease());
                }
                if terminal {
                    let order_id = sqlx::query_scalar::<_, Uuid>(
                        "UPDATE commerce.fulfillments SET status = 'delivered', \
                                delivered_at = $1, updated_at = $2 \
                         WHERE store_id = $3 AND id = $4 \
                           AND status = 'shipped' RETURNING order_id",
                    )
                    .bind(snapshot.observed_at)
                    .bind(now)
                    .bind(job.store_id.as_uuid())
                    .bind(job.fulfillment_id.as_uuid())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(database_error)?;
                    if let Some(order_id) = order_id {
                        sqlx::query(
                            "INSERT INTO integration.event_outbox \
                             (id, store_id, aggregate_type, aggregate_id, \
                              event_type, payload) \
                             VALUES (uuidv7(), $1, 'fulfillment', $2, \
                                     'fulfillment.delivered', $3)",
                        )
                        .bind(job.store_id.as_uuid())
                        .bind(job.fulfillment_id.as_uuid())
                        .bind(json!({
                            "fulfillment_id": job.fulfillment_id.as_uuid(),
                            "order_id": order_id,
                            "status": "delivered",
                        }))
                        .execute(&mut *transaction)
                        .await
                        .map_err(database_error)?;
                    }
                }
            }
            Err(error) => {
                let error = bounded_error(error);
                let result = sqlx::query(
                    "UPDATE commerce.shipping_labels SET next_tracking_refresh_at = CASE \
                            WHEN tracking_attempts >= 8 THEN NULL \
                            ELSE $1 + make_interval(secs => least(power(2, \
                                greatest(tracking_attempts - 1, 0))::integer, 3600)) END, \
                            tracking_locked_by = NULL, tracking_locked_at = NULL, \
                            tracking_last_error = $2, updated_at = $3 \
                     WHERE store_id = $4 AND id = $5 \
                       AND tracking_locked_by = $6",
                )
                .bind(job.store_id.as_uuid())
                .bind(job.label_id.as_uuid())
                .bind(worker_id)
                .bind(now)
                .bind(error)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                if result.rows_affected() != 1 {
                    return Err(stale_tracking_lease());
                }
            }
        }
        transaction.commit().await.map_err(database_error)
    }

    async fn claim_cancellations(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<ShippingCancellationJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, CancellationClaimRow>(
            "SELECT label_id, store_id, fulfillment_id, provider, \
                    provider_shipment_reference, credential_secret_reference, attempts \
             FROM commerce.claim_shipping_cancellations($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(ShippingCancellationJob {
                    label_id: ShippingLabelId::from_uuid(row.label_id),
                    store_id: StoreId::from_uuid(row.store_id),
                    fulfillment_id: FulfillmentId::from_uuid(row.fulfillment_id),
                    provider: row.provider,
                    provider_shipment_reference: row.provider_shipment_reference,
                    credential_secret_reference: ShippingSecretReference::new(
                        row.credential_secret_reference,
                    )?,
                    attempts: u32::try_from(row.attempts).map_err(conversion_error)?,
                })
            })
            .collect()
    }

    async fn finish_cancellation(
        &self,
        worker_id: Uuid,
        job: &ShippingCancellationJob,
        result: Result<ShippingCancellationStatus, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let updated = match result {
            Ok(status) => sqlx::query(
                "UPDATE commerce.shipping_labels SET cancellation_status = $1, \
                        cancellation_reconcile_at = CASE WHEN $2 = 'submitted' \
                            THEN $3 + INTERVAL '15 minutes' ELSE NULL END, \
                        cancellation_locked_by = NULL, cancellation_locked_at = NULL, \
                        cancellation_attempts = 0, cancellation_last_error = NULL, updated_at = $4 \
                 WHERE store_id = $5 AND id = $6 \
                   AND cancellation_locked_by = $7",
            )
            .bind(cancellation_status_text(status))
            .bind(cancellation_status_text(status))
            .bind(now)
            .bind(now)
            .bind(job.store_id.as_uuid())
            .bind(job.label_id.as_uuid())
            .bind(worker_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?,
            Err(error) => sqlx::query(
                "UPDATE commerce.shipping_labels SET cancellation_reconcile_at = CASE \
                        WHEN cancellation_attempts >= 8 THEN NULL \
                        ELSE $1 + make_interval(secs => least(power(2, \
                            greatest(cancellation_attempts - 1, 0))::integer, 3600)) END, \
                        cancellation_locked_by = NULL, cancellation_locked_at = NULL, \
                        cancellation_last_error = $2, updated_at = $3 \
                 WHERE store_id = $4 AND id = $5 \
                   AND cancellation_locked_by = $6",
            )
            .bind(now)
            .bind(bounded_error(error))
            .bind(now)
            .bind(job.store_id.as_uuid())
            .bind(job.label_id.as_uuid())
            .bind(worker_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?,
        };
        if updated.rows_affected() != 1 {
            return Err(stale_cancellation_lease());
        }
        transaction.commit().await.map_err(database_error)
    }
}

async fn load_quote_context(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
    provider_account_id: ShippingProviderAccountId,
) -> Result<QuoteContextRow, ApplicationError> {
    sqlx::query_as::<_, QuoteContextRow>(
        "SELECT account.provider, account.credential_secret_reference, \
                account.origin_name, account.origin_company, account.origin_address_line_1, \
                account.origin_address_line_2, account.origin_city, account.origin_region, \
                account.origin_postal_code, account.origin_country_code::text, \
                account.origin_phone, account.origin_email, order_record.shipping_full_name AS destination_name, \
                NULL::text AS destination_company, \
                order_record.shipping_address_line1 AS destination_address_line_1, \
                order_record.shipping_address_line2 AS destination_address_line_2, \
                order_record.shipping_locality AS destination_city, \
                order_record.shipping_administrative_area AS destination_region, \
                order_record.shipping_postal_code AS destination_postal_code, \
                order_record.shipping_country_code::text AS destination_country_code, \
                order_record.contact_phone AS destination_phone, order_record.contact_email::text AS destination_email \
         FROM commerce.fulfillments AS fulfillment \
         INNER JOIN commerce.shipping_provider_accounts AS account \
            ON account.store_id = fulfillment.store_id AND account.id = $1 \
         INNER JOIN commerce.orders AS order_record \
            ON order_record.store_id = fulfillment.store_id AND order_record.id = fulfillment.order_id \
         WHERE fulfillment.store_id = $2 \
           AND fulfillment.id = $3 AND fulfillment.status = 'pending' \
           AND account.enabled AND order_record.shipping_postal_code IS NOT NULL",
    )
    .bind(provider_account_id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or(ApplicationError::Conflict {
        code: "shipping_quote_context_unavailable",
        message: "the Fulfillment, enabled Provider, or complete shipping address is unavailable",
    })
}

async fn load_rates(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    quote_request_id: ShippingQuoteRequestId,
) -> Result<Vec<ShippingRateQuoteDetail>, ApplicationError> {
    let rows = sqlx::query_as::<_, RateRow>(
        "SELECT id, quote_request_id, provider_shipment_reference, provider_rate_reference, \
                carrier, service, amount_minor, currency::text, estimated_delivery_days, \
                guaranteed, expires_at \
         FROM commerce.shipping_rate_quotes \
         WHERE store_id = $1 AND quote_request_id = $2 \
         ORDER BY amount_minor, carrier, service, id",
    )
    .bind(store_id.as_uuid())
    .bind(quote_request_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ShippingRateQuoteDetail {
                id: ShippingRateQuoteId::from_uuid(row.id),
                quote_request_id: ShippingQuoteRequestId::from_uuid(row.quote_request_id),
                rate: ShippingRateQuote {
                    provider_shipment_reference: row.provider_shipment_reference,
                    provider_rate_reference: row.provider_rate_reference,
                    carrier: row.carrier,
                    service: row.service,
                    amount_minor: row.amount_minor,
                    currency: CurrencyCode::parse(&row.currency)?,
                    estimated_delivery_days: row
                        .estimated_delivery_days
                        .map(u16::try_from)
                        .transpose()
                        .map_err(conversion_error)?,
                    guaranteed: row.guaranteed,
                },
                expires_at: row.expires_at,
            })
        })
        .collect()
}

fn origin_address(row: &QuoteContextRow) -> ShippingAddress {
    ShippingAddress {
        name: row.origin_name.clone(),
        company: row.origin_company.clone(),
        address_line_1: row.origin_address_line_1.clone(),
        address_line_2: row.origin_address_line_2.clone(),
        city: row.origin_city.clone(),
        region: row.origin_region.clone(),
        postal_code: row.origin_postal_code.clone(),
        country_code: row.origin_country_code.clone(),
        phone: row.origin_phone.clone(),
        email: row.origin_email.clone(),
    }
}

fn destination_address(row: &QuoteContextRow) -> ShippingAddress {
    ShippingAddress {
        name: row.destination_name.clone(),
        company: row.destination_company.clone(),
        address_line_1: row.destination_address_line_1.clone(),
        address_line_2: row.destination_address_line_2.clone(),
        city: row.destination_city.clone(),
        region: row.destination_region.clone(),
        postal_code: row.destination_postal_code.clone(),
        country_code: row.destination_country_code.clone(),
        phone: row.destination_phone.clone(),
        email: Some(row.destination_email.clone()),
    }
}

fn validate_parcel(parcel: ShippingParcel) -> Result<(), ApplicationError> {
    if [
        parcel.length_millimetres,
        parcel.width_millimetres,
        parcel.height_millimetres,
    ]
    .into_iter()
    .any(|dimension| !(1..=10_000).contains(&dimension))
        || !(1..=1_000_000).contains(&parcel.weight_grams)
    {
        return Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "parcel",
                reason: "dimensions and weight must be positive and within shipping limits".into(),
            }],
        });
    }
    Ok(())
}

fn cancellation_status_text(status: ShippingCancellationStatus) -> &'static str {
    match status {
        ShippingCancellationStatus::Submitted => "submitted",
        ShippingCancellationStatus::Cancelled => "cancelled",
        ShippingCancellationStatus::Rejected => "rejected",
        ShippingCancellationStatus::NotAvailable => "not_available",
    }
}

fn parse_cancellation_status(value: &str) -> Option<ShippingCancellationStatus> {
    match value {
        "submitted" => Some(ShippingCancellationStatus::Submitted),
        "cancelled" => Some(ShippingCancellationStatus::Cancelled),
        "rejected" => Some(ShippingCancellationStatus::Rejected),
        "not_available" => Some(ShippingCancellationStatus::NotAvailable),
        _ => None,
    }
}

fn tracking_status_text(status: chaos_application::ports::ProviderTrackingStatus) -> &'static str {
    use chaos_application::ports::ProviderTrackingStatus;
    match status {
        ProviderTrackingStatus::PreTransit => "pre_transit",
        ProviderTrackingStatus::InTransit => "in_transit",
        ProviderTrackingStatus::OutForDelivery => "out_for_delivery",
        ProviderTrackingStatus::Delivered => "delivered",
        ProviderTrackingStatus::Failure => "failure",
        ProviderTrackingStatus::Unknown => "unknown",
    }
}

fn tracking_status(value: &str) -> Option<chaos_application::ports::ProviderTrackingStatus> {
    use chaos_application::ports::ProviderTrackingStatus;
    match value {
        "pre_transit" => Some(ProviderTrackingStatus::PreTransit),
        "in_transit" => Some(ProviderTrackingStatus::InTransit),
        "out_for_delivery" => Some(ProviderTrackingStatus::OutForDelivery),
        "delivered" => Some(ProviderTrackingStatus::Delivered),
        "failure" => Some(ProviderTrackingStatus::Failure),
        "unknown" => Some(ProviderTrackingStatus::Unknown),
        _ => None,
    }
}

fn bounded_error(error: String) -> String {
    error.chars().take(2000).collect()
}

fn idempotency_reused() -> ApplicationError {
    ApplicationError::Conflict {
        code: "idempotency_key_reused",
        message: "the idempotency key was already used with a different request",
    }
}

fn quote_not_found(id: ShippingQuoteRequestId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_quote_request",
        id: id.as_uuid().to_string(),
    }
}

fn label_not_found(id: ShippingLabelId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_label",
        id: id.as_uuid().to_string(),
    }
}

fn stale_tracking_lease() -> ApplicationError {
    ApplicationError::Conflict {
        code: "shipping_tracking_lease_lost",
        message: "the Shipping Tracking lease is no longer owned by this Worker",
    }
}

fn stale_cancellation_lease() -> ApplicationError {
    ApplicationError::Conflict {
        code: "shipping_cancellation_lease_lost",
        message: "the Shipping Cancellation lease is no longer owned by this Worker",
    }
}

async fn load_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: ShippingProviderAccountId,
) -> Result<Option<ShippingProviderAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, provider, display_name, enabled, \
                credential_secret_reference IS NOT NULL AS credentials_configured, \
                origin_name, origin_company, origin_address_line_1, origin_address_line_2, \
                origin_city, origin_region, origin_postal_code, origin_country_code::text, \
                origin_phone, origin_email, credential_rotation_expires_at, created_at, updated_at \
         FROM commerce.shipping_provider_accounts \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(provider_account_detail)
    .transpose()
}

fn provider_account_detail(
    row: ProviderAccountRow,
) -> Result<ShippingProviderAccountDetail, ApplicationError> {
    Ok(ShippingProviderAccountDetail {
        account: ShippingProviderAccount::rehydrate(
            ShippingProviderAccountId::from_uuid(row.id),
            row.provider,
            row.display_name,
            row.enabled,
        )?,
        credentials_configured: row.credentials_configured,
        origin: ShippingAddress {
            name: row.origin_name,
            company: row.origin_company,
            address_line_1: row.origin_address_line_1,
            address_line_2: row.origin_address_line_2,
            city: row.origin_city,
            region: row.origin_region,
            postal_code: row.origin_postal_code,
            country_code: row.origin_country_code,
            phone: row.origin_phone,
            email: row.origin_email,
        },
        credential_rotation_expires_at: row.credential_rotation_expires_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store_id: StoreId,
    service_id: ShippingServiceId,
) -> Result<Option<ShippingServiceDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, ServiceRow>(
        "SELECT id, code, name, amount_minor, currency::text, estimated_min_days, \
                estimated_max_days, status::text, created_at, updated_at \
         FROM commerce.shipping_services \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(service_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    match row {
        Some(row) => Ok(Some(detail(transaction, actor, store_id, row).await?)),
        None => Ok(None),
    }
}

async fn detail(
    transaction: &mut Transaction<'static, Postgres>,
    _actor: &AdminActor,
    store_id: StoreId,
    row: ServiceRow,
) -> Result<ShippingServiceDetail, ApplicationError> {
    let countries = sqlx::query_scalar::<_, String>(
        "SELECT country_code::text FROM commerce.shipping_service_regions \
         WHERE store_id = $1 AND shipping_service_id = $2 \
         ORDER BY country_code",
    )
    .bind(store_id.as_uuid())
    .bind(row.0)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let min = u16::try_from(row.5).map_err(conversion_error)?;
    let max = u16::try_from(row.6).map_err(conversion_error)?;
    let service = ShippingService::rehydrate(
        ShippingServiceId::from_uuid(row.0),
        row.1,
        row.2,
        Money::new(row.3, CurrencyCode::parse(&row.4)?),
        min,
        max,
        countries,
        ShippingServiceStatus::parse(&row.7).ok_or_else(corrupt_state)?,
    )?;
    Ok(ShippingServiceDetail {
        service,
        created_at: row.8,
        updated_at: row.9,
    })
}

async fn require_store_currency(
    transaction: &mut Transaction<'static, Postgres>,
    _actor: &AdminActor,
    store_id: StoreId,
    currency: CurrencyCode,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.stores s \
         JOIN commerce.store_currencies c ON c.store_id = s.id \
         WHERE s.id = $1 \
           AND c.currency = $2 AND c.enabled)",
    )
    .bind(store_id.as_uuid())
    .bind(currency.as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if exists {
        Ok(())
    } else {
        Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "currency",
                reason: "must be enabled for the Store".into(),
            }],
        })
    }
}

async fn require_store(
    transaction: &mut Transaction<'static, Postgres>,
    _actor: &AdminActor,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1)")
            .bind(store_id.as_uuid())
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?;
    if exists {
        Ok(())
    } else {
        Err(ApplicationError::NotFound {
            resource: "store",
            id: store_id.as_uuid().to_string(),
        })
    }
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(body) = idempotency::reserve(
        transaction,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
    )
    .await?
    else {
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
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ShippingServiceId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
        200,
        json!({ "data": { "id": id.as_uuid() } }),
    )
    .await
}

async fn complete_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ShippingProviderAccountId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
        200,
        json!({ "data": { "id": id.as_uuid() } }),
    )
    .await
}

fn provider_account_not_found(id: ShippingProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_provider_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Shipping Provider Account state"
    ))
}

fn service_not_found(id: ShippingServiceId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_service",
        id: id.as_uuid().to_string(),
    }
}
fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Shipping Service state"
    ))
}
fn conversion_error(error: std::num::TryFromIntError) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn map_create_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error
        && error.constraint() == Some("shipping_services_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "shipping_service_code_taken",
            message: "the Shipping Service code is already in use for this Store",
        };
    }
    database_error(error)
}

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error
        && error.constraint() == Some("shipping_provider_accounts_store_provider_key")
    {
        return ApplicationError::Conflict {
            code: "shipping_provider_already_configured",
            message: "the Shipping Provider is already configured for this Store",
        };
    }
    database_error(error)
}

fn map_label_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error
        && matches!(
            error.constraint(),
            Some("shipping_labels_purchase_idempotency_key")
                | Some("shipping_labels_cancellation_idempotency_key")
        )
    {
        return idempotency_reused();
    }
    database_error(error)
}
