use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{AnalyticsEventRepository, MachineActor},
};
use chaos_domain::analytics::{BrowserEvent, BrowserEventProperties};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct PostgresAnalyticsEventRepository {
    pool: PgPool,
}

impl PostgresAnalyticsEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AnalyticsEventRepository for PostgresAnalyticsEventRepository {
    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        collection_policy_version: &str,
        received_at: OffsetDateTime,
        retention_expires_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError> {
        let sales_channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let context_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores AS store \
             INNER JOIN merchant.sales_channels AS channel \
               ON channel.merchant_account_id = store.merchant_account_id \
              AND channel.store_id = store.id \
             WHERE store.merchant_account_id = $1 AND store.id = $2 \
               AND store.status = 'active' AND channel.id = $3 AND channel.status = 'active')",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !context_exists {
            return Err(ApplicationError::Forbidden);
        }

        let mut stored = 0;
        for event in events {
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO analytics.behavior_events \
                 (id, event_id, merchant_account_id, store_id, sales_channel_id, event_name, \
                  schema_version, source, anonymous_id, session_id, analytics_storage_consent, \
                  advertising_storage_consent, consent_policy_version, \
                  collection_policy_version, properties, occurred_at, received_at, \
                  retention_expires_at) \
                 VALUES (uuidv7(),$1,$2,$3,$4,$5::analytics.browser_event_name,$6, \
                         'browser',$7,$8,true,$9,$10,$11,$12,$13,$14,$15) \
                 ON CONFLICT (merchant_account_id, store_id, event_id) DO NOTHING \
                 RETURNING id",
            )
            .bind(event.event_id())
            .bind(actor.merchant_account_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(sales_channel_id.as_uuid())
            .bind(event.name().as_str())
            .bind(i16::try_from(event.schema_version()).map_err(conversion_error)?)
            .bind(event.anonymous_id())
            .bind(event.session_id())
            .bind(event.consent().advertising_storage())
            .bind(event.consent().policy_version())
            .bind(collection_policy_version)
            .bind(properties(event.properties()))
            .bind(event.occurred_at())
            .bind(received_at)
            .bind(retention_expires_at)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
            stored += usize::from(inserted.is_some());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(stored)
    }
}

fn properties(value: &BrowserEventProperties) -> Value {
    match value {
        BrowserEventProperties::PageViewed {
            path,
            title,
            referrer_domain,
        } => json!({
            "path": path,
            "title": title,
            "referrer_domain": referrer_domain,
        }),
        BrowserEventProperties::ProductViewed {
            product_id,
            product_variant_id,
        } => json!({
            "product_id": product_id.as_uuid(),
            "product_variant_id": product_variant_id.map(|id| id.as_uuid()),
        }),
        BrowserEventProperties::SearchPerformed {
            query,
            result_count,
        } => json!({
            "query": query,
            "result_count": result_count,
        }),
        BrowserEventProperties::CartLineAdded {
            cart_id,
            product_variant_id,
            quantity,
        } => json!({
            "cart_id": cart_id.as_uuid(),
            "product_variant_id": product_variant_id.as_uuid(),
            "quantity": quantity,
        }),
        BrowserEventProperties::CheckoutStarted {
            cart_id,
            checkout_id,
        } => json!({
            "cart_id": cart_id.as_uuid(),
            "checkout_id": checkout_id.map(|id| id.as_uuid()),
        }),
        BrowserEventProperties::EngagementHeartbeat {
            page_view_event_id,
            active_milliseconds,
        } => json!({
            "page_view_event_id": page_view_event_id,
            "active_milliseconds": active_milliseconds,
        }),
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
