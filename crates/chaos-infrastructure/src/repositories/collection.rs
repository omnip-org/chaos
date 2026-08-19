use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CollectionDetail, CollectionListItem, CollectionProductItem,
        CollectionPublicationRecord, CollectionRepository, CreateCollectionRecord,
        IdempotencyRequest, MachineActor, StorefrontCollectionItem,
    },
};
use chaos_domain::{
    Locale,
    catalog::{CollectionContent, CollectionId, CollectionStatus, ProductId},
    merchant::{SalesChannelId, StoreId},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE: &str = "collections.create.v1";
const UPDATE: &str = "collections.update.v1";
const ACTIVATE: &str = "collections.activate.v1";
const ARCHIVE: &str = "collections.archive.v1";
const REPLACE_PRODUCTS: &str = "collections.replace_products.v1";
const PUBLISH: &str = "collections.publish.v1";
const UNPUBLISH: &str = "collections.unpublish.v1";

#[derive(Clone)]
pub struct PostgresCollectionRepository {
    pool: PgPool,
}

impl PostgresCollectionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }

    async fn begin_storefront(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }
}

#[async_trait]
impl CollectionRepository for PostgresCollectionRepository {
    async fn create(
        &self,
        actor: AdminActor,
        record: CreateCollectionRecord,
        request: &IdempotencyRequest,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, CREATE, request).await? {
            return Ok(CollectionId::from_uuid(id));
        }
        require_store(&mut tx, &actor, record.store_id).await?;
        sqlx::query("INSERT INTO catalog.collections (id, store_id, handle, title, description, metadata, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6::jsonb,'draft',$7,$7)")
            .bind(record.id.as_uuid()).bind(record.store_id.as_uuid())
            .bind(record.content.handle().as_str()).bind(record.content.title()).bind(record.content.description())
            .bind(record.content.metadata().map(chaos_domain::catalog::CatalogMetadata::as_str)).bind(record.created_at)
            .execute(&mut *tx).await.map_err(map_collection_error)?;
        event(
            &mut tx,
            &actor,
            record.store_id,
            record.id,
            "created",
            (None, None),
            record.created_at,
        )
        .await?;
        complete(&mut tx, &actor, CREATE, request, record.id, 201).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(record.id)
    }

    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Option<Vec<CollectionListItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if !store_exists(&mut tx, &actor, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, (Uuid,String,String,String,i64,OffsetDateTime,OffsetDateTime)>(
            "SELECT collection.id, collection.handle::text, collection.title, collection.status::text, count(member.product_id), collection.created_at, collection.updated_at FROM catalog.collections AS collection LEFT JOIN catalog.collection_products AS member ON member.store_id = collection.store_id AND member.collection_id = collection.id WHERE collection.store_id = $1 AND ($2::uuid IS NULL OR collection.id > $2) GROUP BY collection.id ORDER BY collection.id LIMIT $3")
            .bind(store_id.as_uuid()).bind(after.map(CollectionId::as_uuid)).bind(i64::from(limit))
            .fetch_all(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(CollectionListItem {
                    id: CollectionId::from_uuid(row.0),
                    handle: row.1,
                    title: row.2,
                    status: parse_status(&row.3)?,
                    product_count: u32::try_from(row.4).map_err(|_| invalid_snapshot())?,
                    created_at: row.5,
                    updated_at: row.6,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
    ) -> Result<Option<CollectionDetail>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let header = sqlx::query_as::<_, (Uuid,String,String,String,String,Option<serde_json::Value>,OffsetDateTime,OffsetDateTime)>("SELECT id, handle::text, title, description, status::text, metadata, created_at, updated_at FROM catalog.collections WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).fetch_optional(&mut *tx).await.map_err(database_error)?;
        let Some(row) = header else {
            tx.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let products = sqlx::query_as::<_, (Uuid,i32)>("SELECT product_id, position FROM catalog.collection_products WHERE store_id=$1 AND collection_id=$2 ORDER BY position")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).fetch_all(&mut *tx).await.map_err(database_error)?;
        let channels = sqlx::query_scalar::<_,Uuid>("SELECT sales_channel_id FROM catalog.collection_publications WHERE store_id=$1 AND collection_id=$2 ORDER BY sales_channel_id")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).fetch_all(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(Some(CollectionDetail {
            id: CollectionId::from_uuid(row.0),
            handle: row.1,
            title: row.2,
            description: row.3,
            status: parse_status(&row.4)?,
            products: products
                .into_iter()
                .map(|(id, position)| {
                    Ok(CollectionProductItem {
                        product_id: ProductId::from_uuid(id),
                        position: u32::try_from(position).map_err(|_| invalid_snapshot())?,
                    })
                })
                .collect::<Result<_, ApplicationError>>()?,
            published_sales_channel_ids: channels
                .into_iter()
                .map(SalesChannelId::from_uuid)
                .collect(),
            metadata: row.5,
            created_at: row.6,
            updated_at: row.7,
        }))
    }

    async fn update(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        content: &CollectionContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, UPDATE, request).await? {
            return Ok(CollectionId::from_uuid(id));
        }
        require_writable_collection(&mut tx, &actor, store_id, collection_id).await?;
        let changed=sqlx::query("UPDATE catalog.collections SET handle=$3,title=$4,description=$5,metadata=$6::jsonb,updated_at=$7 WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(content.handle().as_str()).bind(content.title()).bind(content.description()).bind(content.metadata().map(chaos_domain::catalog::CatalogMetadata::as_str)).bind(now).execute(&mut *tx).await.map_err(map_collection_error)?.rows_affected();
        require_changed(changed, collection_id)?;
        event(
            &mut tx,
            &actor,
            store_id,
            collection_id,
            "updated",
            (None, None),
            now,
        )
        .await?;
        complete(&mut tx, &actor, UPDATE, request, collection_id, 200).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn set_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        status: CollectionStatus,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let operation = if status == CollectionStatus::Active {
            ACTIVATE
        } else {
            ARCHIVE
        };
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, operation, request).await? {
            return Ok(CollectionId::from_uuid(id));
        }
        let changed=sqlx::query("UPDATE catalog.collections SET status=$3::catalog.collection_status,updated_at=$4 WHERE store_id=$1 AND id=$2 AND (($3='active' AND status='draft') OR ($3='archived' AND status IN ('draft','active')))")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(status.as_str()).bind(now).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if changed == 0 && !collection_exists(&mut tx, &actor, store_id, collection_id).await? {
            return Err(not_found(collection_id));
        }
        if changed == 0 && status == CollectionStatus::Active {
            let current: String = sqlx::query_scalar(
                "SELECT status::text FROM catalog.collections WHERE store_id=$1 AND id=$2",
            )
            .bind(store_id.as_uuid())
            .bind(collection_id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
            if current == "archived" {
                return Err(ApplicationError::Conflict {
                    code: "collection_archived",
                    message: "an archived Collection cannot be reactivated",
                });
            }
        }
        if changed == 1 {
            event(
                &mut tx,
                &actor,
                store_id,
                collection_id,
                if status == CollectionStatus::Active {
                    "activated"
                } else {
                    "archived"
                },
                (None, None),
                now,
            )
            .await?;
        }
        complete(&mut tx, &actor, operation, request, collection_id, 200).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn replace_products(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        product_ids: &[ProductId],
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, REPLACE_PRODUCTS, request).await? {
            return Ok(CollectionId::from_uuid(id));
        }
        require_writable_collection(&mut tx, &actor, store_id, collection_id).await?;
        let ids: Vec<Uuid> = product_ids.iter().map(|id| id.as_uuid()).collect();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog.products WHERE store_id=$1 AND id=ANY($2::uuid[])",
        )
        .bind(store_id.as_uuid())
        .bind(&ids)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if usize::try_from(count).ok() != Some(ids.len()) {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "product_ids",
                    reason: "must all identify Products in the same Store".into(),
                }],
            });
        }
        sqlx::query(
            "DELETE FROM catalog.collection_products WHERE store_id=$1 AND collection_id=$2",
        )
        .bind(store_id.as_uuid())
        .bind(collection_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        for (position, id) in ids.iter().enumerate() {
            sqlx::query("INSERT INTO catalog.collection_products (store_id,collection_id,product_id,position,created_at) VALUES ($1,$2,$3,$4,$5)").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(id).bind(i32::try_from(position).map_err(|_|invalid_snapshot())?).bind(now).execute(&mut *tx).await.map_err(database_error)?;
        }
        sqlx::query("UPDATE catalog.collections SET updated_at=$3 WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid())
            .bind(collection_id.as_uuid())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        event(
            &mut tx,
            &actor,
            store_id,
            collection_id,
            "products_replaced",
            (
                None,
                Some(i32::try_from(ids.len()).map_err(|_| invalid_snapshot())?),
            ),
            now,
        )
        .await?;
        complete(
            &mut tx,
            &actor,
            REPLACE_PRODUCTS,
            request,
            collection_id,
            200,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn set_publication(
        &self,
        actor: AdminActor,
        record: CollectionPublicationRecord,
        request: &IdempotencyRequest,
    ) -> Result<CollectionId, ApplicationError> {
        let CollectionPublicationRecord {
            store_id,
            collection_id,
            sales_channel_id,
            published,
            changed_at: now,
        } = record;
        let operation = if published { PUBLISH } else { UNPUBLISH };
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, operation, request).await? {
            return Ok(CollectionId::from_uuid(id));
        }
        let status: String = sqlx::query_scalar(
            "SELECT status::text FROM catalog.collections WHERE store_id=$1 AND id=$2",
        )
        .bind(store_id.as_uuid())
        .bind(collection_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| not_found(collection_id))?;
        if published && status != "active" {
            return Err(ApplicationError::Conflict {
                code: "collection_not_active",
                message: "the Collection must be active before publication",
            });
        }
        let channel:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM merchant.sales_channels WHERE store_id=$1 AND id=$2 AND status='active')").bind(store_id.as_uuid()).bind(sales_channel_id.as_uuid()).fetch_one(&mut *tx).await.map_err(database_error)?;
        if published && !channel {
            return Err(ApplicationError::NotFound {
                resource: "sales_channel",
                id: sales_channel_id.as_uuid().to_string(),
            });
        }
        let changed = if published {
            sqlx::query("INSERT INTO catalog.collection_publications (store_id,collection_id,sales_channel_id,published_at) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(sales_channel_id.as_uuid()).bind(now).execute(&mut *tx).await.map_err(database_error)?.rows_affected()
        } else {
            sqlx::query("DELETE FROM catalog.collection_publications WHERE store_id=$1 AND collection_id=$2 AND sales_channel_id=$3").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(sales_channel_id.as_uuid()).execute(&mut *tx).await.map_err(database_error)?.rows_affected()
        };
        if changed == 1 {
            event(
                &mut tx,
                &actor,
                store_id,
                collection_id,
                if published {
                    "published"
                } else {
                    "unpublished"
                },
                (Some(sales_channel_id), None),
                now,
            )
            .await?;
        }
        complete(&mut tx, &actor, operation, request, collection_id, 200).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn list_storefront(
        &self,
        actor: &MachineActor,
        requested_locale: Option<Locale>,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCollectionItem>, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.begin_storefront(actor).await?;
        let locale = resolve_collection_locale(&mut tx, actor, requested_locale).await?;
        let rows=sqlx::query_as::<_,(Uuid,String,String,String,Option<serde_json::Value>,i64)>("SELECT collection.id,collection.handle::text,COALESCE((SELECT translation.title FROM catalog.collection_translations AS translation WHERE translation.store_id=collection.store_id AND translation.collection_id=collection.id AND (translation.locale=$5 OR translation.locale=$6) ORDER BY CASE WHEN translation.locale=$5 THEN 0 ELSE 1 END LIMIT 1),collection.title),COALESCE((SELECT translation.description FROM catalog.collection_translations AS translation WHERE translation.store_id=collection.store_id AND translation.collection_id=collection.id AND (translation.locale=$5 OR translation.locale=$6) ORDER BY CASE WHEN translation.locale=$5 THEN 0 ELSE 1 END LIMIT 1),collection.description),collection.metadata,count(member.product_id) FILTER (WHERE product.status='active' AND product_publication.product_id IS NOT NULL) FROM catalog.collections AS collection INNER JOIN catalog.collection_publications AS publication ON publication.store_id=collection.store_id AND publication.collection_id=collection.id AND publication.sales_channel_id=$2 INNER JOIN merchant.stores AS store ON store.id=collection.store_id AND store.status='active' INNER JOIN merchant.sales_channels AS channel ON channel.store_id=collection.store_id AND channel.id=$2 AND channel.status='active' LEFT JOIN catalog.collection_products AS member ON member.store_id=collection.store_id AND member.collection_id=collection.id LEFT JOIN catalog.products AS product ON product.store_id=member.store_id AND product.id=member.product_id LEFT JOIN catalog.product_publications AS product_publication ON product_publication.store_id=product.store_id AND product_publication.product_id=product.id AND product_publication.sales_channel_id=$2 WHERE collection.store_id=$1 AND collection.status='active' AND ($3::uuid IS NULL OR collection.id>$3) GROUP BY collection.id ORDER BY collection.id LIMIT $4")
            .bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).bind(after.map(CollectionId::as_uuid)).bind(i64::from(limit)).bind(locale.1.as_deref()).bind(locale.2.as_deref()).fetch_all(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(|row| storefront_item(row, locale.0.clone()))
            .collect()
    }

    async fn get_storefront_by_handle(
        &self,
        actor: &MachineActor,
        requested_locale: Option<Locale>,
        handle: &str,
    ) -> Result<Option<StorefrontCollectionItem>, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.begin_storefront(actor).await?;
        let locale = resolve_collection_locale(&mut tx, actor, requested_locale).await?;
        let row=sqlx::query_as::<_,(Uuid,String,String,String,Option<serde_json::Value>,i64)>("SELECT collection.id,collection.handle::text,COALESCE((SELECT translation.title FROM catalog.collection_translations AS translation WHERE translation.store_id=collection.store_id AND translation.collection_id=collection.id AND (translation.locale=$4 OR translation.locale=$5) ORDER BY CASE WHEN translation.locale=$4 THEN 0 ELSE 1 END LIMIT 1),collection.title),COALESCE((SELECT translation.description FROM catalog.collection_translations AS translation WHERE translation.store_id=collection.store_id AND translation.collection_id=collection.id AND (translation.locale=$4 OR translation.locale=$5) ORDER BY CASE WHEN translation.locale=$4 THEN 0 ELSE 1 END LIMIT 1),collection.description),collection.metadata,count(member.product_id) FILTER (WHERE product.status='active' AND product_publication.product_id IS NOT NULL) FROM catalog.collections AS collection INNER JOIN catalog.collection_publications AS publication ON publication.store_id=collection.store_id AND publication.collection_id=collection.id AND publication.sales_channel_id=$2 INNER JOIN merchant.stores AS store ON store.id=collection.store_id AND store.status='active' INNER JOIN merchant.sales_channels AS channel ON channel.store_id=collection.store_id AND channel.id=$2 AND channel.status='active' LEFT JOIN catalog.collection_products AS member ON member.store_id=collection.store_id AND member.collection_id=collection.id LEFT JOIN catalog.products AS product ON product.store_id=member.store_id AND product.id=member.product_id LEFT JOIN catalog.product_publications AS product_publication ON product_publication.store_id=product.store_id AND product_publication.product_id=product.id AND product_publication.sales_channel_id=$2 WHERE collection.store_id=$1 AND collection.status='active' AND collection.handle=$3 GROUP BY collection.id")
            .bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).bind(handle).bind(locale.1.as_deref()).bind(locale.2.as_deref()).fetch_optional(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        row.map(|row| storefront_item(row, locale.0)).transpose()
    }
}

async fn resolve_collection_locale(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MachineActor,
    requested: Option<Locale>,
) -> Result<(Locale, Option<String>, Option<String>), ApplicationError> {
    let default: Option<String> =
        sqlx::query_scalar("SELECT default_locale FROM merchant.stores WHERE id=$1")
            .bind(actor.store_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(database_error)?;
    let default = default.ok_or_else(|| ApplicationError::NotFound {
        resource: "store",
        id: actor.store_id.as_uuid().to_string(),
    })?;
    let selected = requested.unwrap_or(Locale::parse(&default)?);
    let exact = if selected.as_str() == default {
        None
    } else {
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM merchant.store_locales WHERE store_id=$1 AND locale=$2)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(selected.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        if !enabled {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "locale",
                    reason: "must be enabled for the Store".into(),
                }],
            });
        }
        Some(selected.as_str().to_owned())
    };
    let language = selected.language();
    let primary = if exact.is_some() && language != selected.as_str() && language != default {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM merchant.store_locales WHERE store_id=$1 AND locale=$2)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(language)
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?
        .then(|| language.to_owned())
    } else {
        None
    };
    Ok((selected, exact, primary))
}

fn storefront_item(
    row: (Uuid, String, String, String, Option<serde_json::Value>, i64),
    locale: Locale,
) -> Result<StorefrontCollectionItem, ApplicationError> {
    Ok(StorefrontCollectionItem {
        id: CollectionId::from_uuid(row.0),
        handle: row.1,
        title: row.2,
        description: row.3,
        product_count: u32::try_from(row.5).map_err(|_| invalid_snapshot())?,
        locale,
        metadata: row.4,
    })
}
async fn require_store(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<(), ApplicationError> {
    if store_exists(tx, actor, store).await? {
        Ok(())
    } else {
        Err(ApplicationError::NotFound {
            resource: "store",
            id: store.as_uuid().to_string(),
        })
    }
}
async fn store_exists(
    tx: &mut Transaction<'_, Postgres>,
    _actor: &AdminActor,
    store: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM merchant.stores WHERE id=$1)")
        .bind(store.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)
}
async fn collection_exists(
    tx: &mut Transaction<'_, Postgres>,
    _actor: &AdminActor,
    store: StoreId,
    id: CollectionId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM catalog.collections WHERE store_id=$1 AND id=$2)",
    )
    .bind(store.as_uuid())
    .bind(id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
}
async fn require_writable_collection(
    tx: &mut Transaction<'_, Postgres>,
    _actor: &AdminActor,
    store: StoreId,
    id: CollectionId,
) -> Result<(), ApplicationError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status::text FROM catalog.collections WHERE store_id=$1 AND id=$2",
    )
    .bind(store.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    match status.as_deref() {
        None => Err(not_found(id)),
        Some("archived") => Err(ApplicationError::Conflict {
            code: "collection_archived",
            message: "an archived Collection cannot be changed",
        }),
        Some(_) => Ok(()),
    }
}
fn require_changed(changed: u64, id: CollectionId) -> Result<(), ApplicationError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(not_found(id))
    }
}
async fn event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    id: CollectionId,
    kind: &str,
    details: (Option<SalesChannelId>, Option<i32>),
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (channel, count) = details;
    sqlx::query("INSERT INTO catalog.collection_events (id,store_id,collection_id,event_kind,actor_user_id,sales_channel_id,product_count,occurred_at) VALUES ($1,$2,$3,$4::catalog.collection_event_kind,$5,$6,$7,$8)").bind(Uuid::now_v7()).bind(store.as_uuid()).bind(id.as_uuid()).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(channel.map(SalesChannelId::as_uuid)).bind(count).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(value) = idempotency::reserve(
        tx,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
    )
    .await?
    else {
        return Ok(None);
    };
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok())
        .map(Some)
        .ok_or_else(invalid_snapshot)
}
async fn complete(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: CollectionId,
    status: i16,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
        status,
        json!({"id":id.as_uuid()}),
    )
    .await
}
fn parse_status(value: &str) -> Result<CollectionStatus, ApplicationError> {
    CollectionStatus::parse(value).ok_or_else(invalid_snapshot)
}
fn not_found(id: CollectionId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "collection",
        id: id.as_uuid().to_string(),
    }
}
fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Collection persistence snapshot is invalid"
    ))
}
fn map_collection_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("collections_store_id_handle_key")
    {
        return ApplicationError::Conflict {
            code: "collection_handle_taken",
            message: "the Collection handle is already in use in this Store",
        };
    }
    database_error(error)
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
