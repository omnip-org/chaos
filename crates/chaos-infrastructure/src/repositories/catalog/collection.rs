use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CollectionDetail, CollectionListItem, CollectionProductItem,
        CollectionPublicationRecord, CollectionRepository, CreateCollectionRecord, MachineActor,
        StorefrontCollectionItem,
    },
};
use chaos_domain::{
    catalog::{CollectionContent, CollectionId, CollectionStatus, ProductId},
    store::{SalesChannelId, StoreId},
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

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
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        require_store(&mut tx, &actor, record.store_id).await?;
        sqlx::query("INSERT INTO commerce.collections (id, store_id, handle, title, description, metadata, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6::jsonb,'draft',$7,$7)")
            .bind(record.id.as_uuid()).bind(record.store_id.as_uuid())
            .bind(record.content.handle().as_str()).bind(record.content.title()).bind(record.content.description())
            .bind(record.content.metadata().map(chaos_domain::catalog::CatalogMetadata::as_str)).bind(record.created_at)
            .execute(&mut *tx).await.map_err(map_collection_error)?;
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
            "SELECT collection.id, collection.handle::text, collection.title, collection.status::text, count(member.product_id), collection.created_at, collection.updated_at FROM commerce.collections AS collection LEFT JOIN commerce.collection_products AS member ON member.store_id = collection.store_id AND member.collection_id = collection.id WHERE collection.store_id = $1 AND ($2::uuid IS NULL OR collection.id > $2) GROUP BY collection.id ORDER BY collection.id LIMIT $3")
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
        let header = sqlx::query_as::<_, (Uuid,String,String,String,String,Option<serde_json::Value>,OffsetDateTime,OffsetDateTime)>("SELECT id, handle::text, title, description, status::text, metadata, created_at, updated_at FROM commerce.collections WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).fetch_optional(&mut *tx).await.map_err(database_error)?;
        let Some(row) = header else {
            tx.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let products = sqlx::query_as::<_, (Uuid,i32)>("SELECT product_id, position FROM commerce.collection_products WHERE store_id=$1 AND collection_id=$2 ORDER BY position")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).fetch_all(&mut *tx).await.map_err(database_error)?;
        let channels = sqlx::query_scalar::<_,Uuid>("SELECT sales_channel_id FROM commerce.collection_publications WHERE store_id=$1 AND collection_id=$2 ORDER BY sales_channel_id")
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
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        require_writable_collection(&mut tx, &actor, store_id, collection_id).await?;
        let changed=sqlx::query("UPDATE commerce.collections SET handle=$3,title=$4,description=$5,metadata=$6::jsonb,updated_at=$7 WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(content.handle().as_str()).bind(content.title()).bind(content.description()).bind(content.metadata().map(chaos_domain::catalog::CatalogMetadata::as_str)).bind(now).execute(&mut *tx).await.map_err(map_collection_error)?.rows_affected();
        require_changed(changed, collection_id)?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn set_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        status: CollectionStatus,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let changed=sqlx::query("UPDATE commerce.collections SET status=$3::commerce.collection_status,updated_at=$4 WHERE store_id=$1 AND id=$2 AND (($3='active' AND status='draft') OR ($3='archived' AND status IN ('draft','active')))")
            .bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(status.as_str()).bind(now).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if changed == 0 && !collection_exists(&mut tx, &actor, store_id, collection_id).await? {
            return Err(not_found(collection_id));
        }
        if changed == 0 && status == CollectionStatus::Active {
            let current: String = sqlx::query_scalar(
                "SELECT status::text FROM commerce.collections WHERE store_id=$1 AND id=$2",
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
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn replace_products(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        product_ids: &[ProductId],
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        require_writable_collection(&mut tx, &actor, store_id, collection_id).await?;
        let ids: Vec<Uuid> = product_ids.iter().map(|id| id.as_uuid()).collect();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.products WHERE store_id=$1 AND id=ANY($2::uuid[])",
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
            "DELETE FROM commerce.collection_products WHERE store_id=$1 AND collection_id=$2",
        )
        .bind(store_id.as_uuid())
        .bind(collection_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        for (position, id) in ids.iter().enumerate() {
            sqlx::query("INSERT INTO commerce.collection_products (store_id,collection_id,product_id,position,created_at) VALUES ($1,$2,$3,$4,$5)").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(id).bind(i32::try_from(position).map_err(|_|invalid_snapshot())?).bind(now).execute(&mut *tx).await.map_err(database_error)?;
        }
        sqlx::query("UPDATE commerce.collections SET updated_at=$3 WHERE store_id=$1 AND id=$2")
            .bind(store_id.as_uuid())
            .bind(collection_id.as_uuid())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn set_publication(
        &self,
        actor: AdminActor,
        record: CollectionPublicationRecord,
    ) -> Result<CollectionId, ApplicationError> {
        let CollectionPublicationRecord {
            store_id,
            collection_id,
            sales_channel_id,
            published,
            changed_at: now,
        } = record;
        let mut tx = self.begin(&actor).await?;
        let status: String = sqlx::query_scalar(
            "SELECT status::text FROM commerce.collections WHERE store_id=$1 AND id=$2",
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
        let channel:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.store_sales_channels WHERE store_id=$1 AND id=$2 AND status='active')").bind(store_id.as_uuid()).bind(sales_channel_id.as_uuid()).fetch_one(&mut *tx).await.map_err(database_error)?;
        if published && !channel {
            return Err(ApplicationError::NotFound {
                resource: "sales_channel",
                id: sales_channel_id.as_uuid().to_string(),
            });
        }
        if published {
            sqlx::query("INSERT INTO commerce.collection_publications (store_id,collection_id,sales_channel_id,published_at) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(sales_channel_id.as_uuid()).bind(now).execute(&mut *tx).await.map_err(database_error)?;
        } else {
            sqlx::query("DELETE FROM commerce.collection_publications WHERE store_id=$1 AND collection_id=$2 AND sales_channel_id=$3").bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(sales_channel_id.as_uuid()).execute(&mut *tx).await.map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(collection_id)
    }

    async fn list_storefront(
        &self,
        actor: &MachineActor,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCollectionItem>, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.begin_storefront(actor).await?;
        let rows=sqlx::query_as::<_,(Uuid,String,String,String,Option<serde_json::Value>,i64)>("SELECT collection.id,collection.handle::text,collection.title,collection.description,collection.metadata,count(member.product_id) FILTER (WHERE product.status='active' AND product_publication.product_id IS NOT NULL) FROM commerce.collections AS collection INNER JOIN commerce.collection_publications AS publication ON publication.store_id=collection.store_id AND publication.collection_id=collection.id AND publication.sales_channel_id=$2 INNER JOIN commerce.stores AS store ON store.id=collection.store_id AND store.status='active' INNER JOIN commerce.store_sales_channels AS channel ON channel.store_id=collection.store_id AND channel.id=$2 AND channel.status='active' LEFT JOIN commerce.collection_products AS member ON member.store_id=collection.store_id AND member.collection_id=collection.id LEFT JOIN commerce.products AS product ON product.store_id=member.store_id AND product.id=member.product_id LEFT JOIN commerce.product_publications AS product_publication ON product_publication.store_id=product.store_id AND product_publication.product_id=product.id AND product_publication.sales_channel_id=$2 WHERE collection.store_id=$1 AND collection.status='active' AND ($3::uuid IS NULL OR collection.id>$3) GROUP BY collection.id ORDER BY collection.id LIMIT $4")
            .bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).bind(after.map(CollectionId::as_uuid)).bind(i64::from(limit)).fetch_all(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter().map(storefront_item).collect()
    }

    async fn get_storefront_by_handle(
        &self,
        actor: &MachineActor,
        handle: &str,
    ) -> Result<Option<StorefrontCollectionItem>, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.begin_storefront(actor).await?;
        let row=sqlx::query_as::<_,(Uuid,String,String,String,Option<serde_json::Value>,i64)>("SELECT collection.id,collection.handle::text,collection.title,collection.description,collection.metadata,count(member.product_id) FILTER (WHERE product.status='active' AND product_publication.product_id IS NOT NULL) FROM commerce.collections AS collection INNER JOIN commerce.collection_publications AS publication ON publication.store_id=collection.store_id AND publication.collection_id=collection.id AND publication.sales_channel_id=$2 INNER JOIN commerce.stores AS store ON store.id=collection.store_id AND store.status='active' INNER JOIN commerce.store_sales_channels AS channel ON channel.store_id=collection.store_id AND channel.id=$2 AND channel.status='active' LEFT JOIN commerce.collection_products AS member ON member.store_id=collection.store_id AND member.collection_id=collection.id LEFT JOIN commerce.products AS product ON product.store_id=member.store_id AND product.id=member.product_id LEFT JOIN commerce.product_publications AS product_publication ON product_publication.store_id=product.store_id AND product_publication.product_id=product.id AND product_publication.sales_channel_id=$2 WHERE collection.store_id=$1 AND collection.status='active' AND collection.handle=$3 GROUP BY collection.id")
            .bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).bind(handle).fetch_optional(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        row.map(storefront_item).transpose()
    }
}

fn storefront_item(
    row: (Uuid, String, String, String, Option<serde_json::Value>, i64),
) -> Result<StorefrontCollectionItem, ApplicationError> {
    Ok(StorefrontCollectionItem {
        id: CollectionId::from_uuid(row.0),
        handle: row.1,
        title: row.2,
        description: row.3,
        product_count: u32::try_from(row.5).map_err(|_| invalid_snapshot())?,
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
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.stores WHERE id=$1)")
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
        "SELECT EXISTS(SELECT 1 FROM commerce.collections WHERE store_id=$1 AND id=$2)",
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
        "SELECT status::text FROM commerce.collections WHERE store_id=$1 AND id=$2",
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
