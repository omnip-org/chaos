use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        CreateStoreDomainRecord, IdempotencyRequest, PendingStoreDomainVerification,
        ResolvedStoreDomain, StoreDomainCreationResult, StoreDomainCreationStatus, StoreDomainItem,
        StoreDomainRepository,
    },
};
use chaos_domain::{
    FieldViolation,
    identity::UserId,
    merchant::{
        MerchantAccountId, SalesChannelId, StoreDomainId, StoreDomainStatus, StoreHostname, StoreId,
    },
};
use serde_json::json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_OPERATION: &str = "store_domains.create.v1";
const VERIFY_OPERATION: &str = "store_domains.verify.v1";
const ARCHIVE_OPERATION: &str = "store_domains.archive.v1";

#[derive(Clone)]
pub struct PostgresStoreDomainRepository {
    pool: PgPool,
}

impl PostgresStoreDomainRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: MerchantActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[derive(FromRow)]
struct DomainRow {
    id: Uuid,
    store_id: Uuid,
    sales_channel_id: Uuid,
    hostname: String,
    domain_status: String,
    created_by: Uuid,
    verified_at: Option<OffsetDateTime>,
    archived_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct PendingDomainRow {
    id: Uuid,
    store_id: Uuid,
    sales_channel_id: Uuid,
    hostname: String,
    domain_status: String,
    created_by: Uuid,
    verified_at: Option<OffsetDateTime>,
    archived_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    verification_token_digest: Vec<u8>,
}

#[async_trait]
impl StoreDomainRepository for PostgresStoreDomainRepository {
    async fn create(
        &self,
        actor: MerchantActor,
        record: CreateStoreDomainRecord,
        request: &IdempotencyRequest,
    ) -> Result<StoreDomainCreationResult, ApplicationError> {
        let CreateStoreDomainRecord {
            store_id,
            domain_id,
            sales_channel_id,
            hostname,
            verification_token_digest,
            created_at: now,
        } = record;
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, CREATE_OPERATION, request).await? {
            let item = load_domain(
                &mut transaction,
                actor,
                store_id,
                StoreDomainId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(StoreDomainCreationResult {
                item: domain_item(item)?,
                status: StoreDomainCreationStatus::Replayed,
            });
        }
        let active_web_channel: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.sales_channels AS channel \
             INNER JOIN merchant.stores AS store \
               ON store.merchant_account_id = channel.merchant_account_id \
              AND store.id = channel.store_id \
             WHERE channel.merchant_account_id = $1 AND channel.store_id = $2 \
               AND channel.id = $3 AND channel.kind = 'web' AND channel.status = 'active' \
               AND store.status IN ('draft', 'active'))",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !active_web_channel {
            return Err(ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "sales_channel_id",
                    reason: "must identify an active Web Sales Channel in a writable Store".into(),
                }],
            });
        }
        sqlx::query(
            "INSERT INTO merchant.store_domains \
             (id, merchant_account_id, store_id, sales_channel_id, hostname, \
              verification_token_digest, domain_status, created_by, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,'pending',$7,$8,$8)",
        )
        .bind(domain_id.as_uuid())
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .bind(hostname.as_str())
        .bind(verification_token_digest.as_slice())
        .bind(actor.user_id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(map_domain_error)?;
        insert_event(&mut transaction, actor, store_id, domain_id, "created", now).await?;
        complete(
            &mut transaction,
            actor,
            CREATE_OPERATION,
            request,
            domain_id,
            201,
        )
        .await?;
        let row = load_domain(&mut transaction, actor, store_id, domain_id)
            .await?
            .ok_or_else(invalid_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(StoreDomainCreationResult {
            item: domain_item(row)?,
            status: StoreDomainCreationStatus::Created,
        })
    }

    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<StoreDomainId>,
        limit: u16,
    ) -> Result<Option<Vec<StoreDomainItem>>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2)",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !exists {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, DomainRow>(
            "SELECT id, store_id, sales_channel_id, hostname::text, domain_status::text, \
                    created_by, verified_at, archived_at, created_at, updated_at \
             FROM merchant.store_domains \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND ($3::uuid IS NULL OR id > $3) ORDER BY id LIMIT $4",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(after.map(StoreDomainId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(domain_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn pending_verification(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
    ) -> Result<PendingStoreDomainVerification, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let row = sqlx::query_as::<_, PendingDomainRow>(
            "SELECT id, store_id, sales_channel_id, hostname::text, domain_status::text, \
                    created_by, verified_at, archived_at, created_at, updated_at, \
                    verification_token_digest \
             FROM merchant.store_domains \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(domain_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| domain_not_found(domain_id))?;
        transaction.commit().await.map_err(database_error)?;
        let item = domain_item(DomainRow {
            id: row.id,
            store_id: row.store_id,
            sales_channel_id: row.sales_channel_id,
            hostname: row.hostname,
            domain_status: row.domain_status,
            created_by: row.created_by,
            verified_at: row.verified_at,
            archived_at: row.archived_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })?;
        if item.domain_status == StoreDomainStatus::Archived {
            return Err(ApplicationError::Conflict {
                code: "store_domain_archived",
                message: "an archived Store Domain cannot be verified",
            });
        }
        let token_digest = row
            .verification_token_digest
            .try_into()
            .map_err(|_| invalid_snapshot())?;
        Ok(PendingStoreDomainVerification {
            verification_required: item.domain_status == StoreDomainStatus::Pending,
            item,
            token_digest,
        })
    }

    async fn mark_verified(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreDomainItem, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, VERIFY_OPERATION, request).await? {
            let row = load_domain(
                &mut transaction,
                actor,
                store_id,
                StoreDomainId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return domain_item(row);
        }
        let changed = sqlx::query(
            "UPDATE merchant.store_domains SET domain_status = 'verified', verified_at = $4, \
                    verified_by = $5, updated_at = $4 \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
               AND domain_status = 'pending'",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(domain_id.as_uuid())
        .bind(now)
        .bind(actor.user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        let row = load_domain(&mut transaction, actor, store_id, domain_id)
            .await?
            .ok_or_else(|| domain_not_found(domain_id))?;
        if changed == 0 && row.domain_status != "verified" {
            return Err(ApplicationError::Conflict {
                code: "store_domain_not_pending",
                message: "the Store Domain is not pending verification",
            });
        }
        if changed == 1 {
            insert_event(
                &mut transaction,
                actor,
                store_id,
                domain_id,
                "verified",
                now,
            )
            .await?;
        }
        complete(
            &mut transaction,
            actor,
            VERIFY_OPERATION,
            request,
            domain_id,
            200,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        domain_item(row)
    }

    async fn archive(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        domain_id: StoreDomainId,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreDomainItem, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, ARCHIVE_OPERATION, request).await? {
            let row = load_domain(
                &mut transaction,
                actor,
                store_id,
                StoreDomainId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return domain_item(row);
        }
        let changed = sqlx::query(
            "UPDATE merchant.store_domains SET domain_status = 'archived', archived_at = $4, \
                    archived_by = $5, updated_at = $4 \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
               AND domain_status <> 'archived'",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(domain_id.as_uuid())
        .bind(now)
        .bind(actor.user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        let row = load_domain(&mut transaction, actor, store_id, domain_id)
            .await?
            .ok_or_else(|| domain_not_found(domain_id))?;
        if changed == 1 {
            insert_event(
                &mut transaction,
                actor,
                store_id,
                domain_id,
                "archived",
                now,
            )
            .await?;
        }
        complete(
            &mut transaction,
            actor,
            ARCHIVE_OPERATION,
            request,
            domain_id,
            200,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        domain_item(row)
    }

    async fn resolve(
        &self,
        hostname: &StoreHostname,
    ) -> Result<Option<ResolvedStoreDomain>, ApplicationError> {
        let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
            "SELECT merchant_account_id, store_id, sales_channel_id, hostname \
             FROM merchant.resolve_store_domain($1)",
        )
        .bind(hostname.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(|row| {
            Ok(ResolvedStoreDomain {
                merchant_account_id: MerchantAccountId::from_uuid(row.0),
                store_id: StoreId::from_uuid(row.1),
                sales_channel_id: SalesChannelId::from_uuid(row.2),
                hostname: StoreHostname::parse(row.3)?,
            })
        })
        .transpose()
    }
}

async fn load_domain(
    transaction: &mut Transaction<'_, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
    domain_id: StoreDomainId,
) -> Result<Option<DomainRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT id, store_id, sales_channel_id, hostname::text, domain_status::text, \
                created_by, verified_at, archived_at, created_at, updated_at \
         FROM merchant.store_domains \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(domain_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

fn domain_item(row: DomainRow) -> Result<StoreDomainItem, ApplicationError> {
    Ok(StoreDomainItem {
        id: StoreDomainId::from_uuid(row.id),
        store_id: StoreId::from_uuid(row.store_id),
        sales_channel_id: SalesChannelId::from_uuid(row.sales_channel_id),
        hostname: StoreHostname::parse(row.hostname)?,
        domain_status: StoreDomainStatus::parse(&row.domain_status).ok_or_else(invalid_snapshot)?,
        created_by: UserId::from_uuid(row.created_by),
        verified_at: row.verified_at,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn insert_event(
    transaction: &mut Transaction<'_, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
    domain_id: StoreDomainId,
    event_kind: &str,
    occurred_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO merchant.store_domain_events \
         (id, merchant_account_id, store_id, store_domain_id, event_kind, actor_user_id, occurred_at) \
         VALUES ($1,$2,$3,$4,$5::merchant.store_domain_event_kind,$6,$7)",
    )
    .bind(Uuid::now_v7())
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(event_kind)
    .bind(actor.user_id().as_uuid())
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(snapshot) = idempotency::reserve(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
    )
    .await?
    else {
        return Ok(None);
    };
    snapshot
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(Some)
        .ok_or_else(invalid_snapshot)
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    domain_id: StoreDomainId,
    status: i16,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
        status,
        json!({"id": domain_id.as_uuid()}),
    )
    .await
}

fn map_domain_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("store_domains_active_hostname_idx")
    {
        return ApplicationError::Conflict {
            code: "store_domain_hostname_taken",
            message: "the hostname is already assigned to a Store",
        };
    }
    database_error(error)
}

fn domain_not_found(domain_id: StoreDomainId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store_domain",
        id: domain_id.as_uuid().to_string(),
    }
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Store Domain persistence snapshot is invalid"
    ))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
