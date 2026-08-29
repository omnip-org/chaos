use std::collections::HashSet;

use crate::{
    ApplicationError,
    catalog::{
        ProductConfigurationDraft, ProductConfigurationOptionInput,
        ProductConfigurationOptionValueInput, ProductConfigurationVariantInput,
    },
    contracts::AdminActor,
    error::database_error,
};
use chaos_domain::catalog::{ProductId, ProductOptionId};
use chaos_domain::store::StoreId;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;

#[derive(Clone)]
pub struct PostgresCatalogConfigurationRepository {
    pool: PgPool,
}

impl PostgresCatalogConfigurationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(crate) async fn sync(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        draft: &ProductConfigurationDraft,
        expected_revision: Option<i64>,
        changed_at: OffsetDateTime,
    ) -> Result<i64, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            actor.audit_user_id(),
            store_id,
        )
        .await
        .map_err(database_error)?;

        let (status, current_revision) = sqlx::query_as::<_, (String, i64)>(
            "SELECT status::text, revision FROM commerce.products \
             WHERE store_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "product",
            id: product_id.as_uuid().to_string(),
        })?;
        ensure_expected_revision(current_revision, expected_revision)?;
        if status == "active" && draft.variants.is_empty() {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "variants",
                    reason: "an active Product must retain at least one active Variant".into(),
                }],
            });
        }
        validate_existing_ids(&mut transaction, store_id, product_id, draft).await?;
        let removed_media_asset_ids =
            archive_removed_media_links(&mut transaction, store_id, product_id, draft, changed_at)
                .await?;

        archive_options(&mut transaction, store_id, product_id, changed_at).await?;
        archive_option_values(&mut transaction, store_id, product_id, changed_at).await?;
        for option in &draft.options {
            upsert_option(&mut transaction, store_id, product_id, option, changed_at).await?;
            for value in &option.values {
                upsert_option_value(
                    &mut transaction,
                    store_id,
                    product_id,
                    option.id,
                    value,
                    changed_at,
                )
                .await?;
            }
        }

        sqlx::query(
            "UPDATE commerce.product_variants \
             SET status='archived', updated_at=$3 \
             WHERE store_id=$1 AND product_id=$2 AND status <> 'archived'",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(changed_at)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        for variant in &draft.variants {
            upsert_variant(&mut transaction, store_id, product_id, variant, changed_at).await?;
            sqlx::query(
                "DELETE FROM commerce.variant_selected_options \
                 WHERE store_id=$1 AND product_id=$2 AND variant_id=$3",
            )
            .bind(store_id.as_uuid())
            .bind(product_id.as_uuid())
            .bind(variant.id.as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            for option in &draft.options {
                let value_id = variant
                    .selected_option_value_ids
                    .iter()
                    .find(|value_id| option.values.iter().any(|value| value.id == **value_id))
                    .copied()
                    .ok_or_else(|| invalid_configuration("selected_option_value_ids"))?;
                sqlx::query(
                    "INSERT INTO commerce.variant_selected_options \
                     (store_id, product_id, variant_id, option_id, option_value_id) \
                     VALUES ($1,$2,$3,$4,$5)",
                )
                .bind(store_id.as_uuid())
                .bind(product_id.as_uuid())
                .bind(variant.id.as_uuid())
                .bind(option.id.as_uuid())
                .bind(value_id.as_uuid())
                .execute(&mut *transaction)
                .await
                .map_err(map_configuration_error)?;
            }
        }

        archive_unreferenced_assets(
            &mut transaction,
            store_id,
            &removed_media_asset_ids,
            changed_at,
        )
        .await?;

        let revision = sqlx::query_scalar::<_, i64>(
            "UPDATE commerce.products \
             SET revision=revision+1, updated_at=$3 \
             WHERE store_id=$1 AND id=$2 \
             RETURNING revision",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(changed_at)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(revision)
    }
}

async fn archive_removed_media_links(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    draft: &ProductConfigurationDraft,
    changed_at: OffsetDateTime,
) -> Result<HashSet<uuid::Uuid>, ApplicationError> {
    let desired_value_ids = draft
        .options
        .iter()
        .flat_map(|option| option.values.iter().map(|value| value.id.as_uuid()))
        .collect::<Vec<_>>();
    let desired_variant_ids = draft
        .variants
        .iter()
        .map(|variant| variant.id.as_uuid())
        .collect::<Vec<_>>();
    let mut removed_media_asset_ids = HashSet::new();
    let option_value_asset_ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "UPDATE commerce.product_option_value_media_assets \
         SET archived_at=$3 \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL \
           AND NOT (option_value_id = ANY($4::uuid[])) \
         RETURNING media_asset_id",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .bind(&desired_value_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    removed_media_asset_ids.extend(option_value_asset_ids);

    let variant_asset_ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "UPDATE commerce.product_variant_media_assets \
         SET archived_at=$3 \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL \
           AND NOT (product_variant_id = ANY($4::uuid[])) \
         RETURNING media_asset_id",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .bind(&desired_variant_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    removed_media_asset_ids.extend(variant_asset_ids);
    Ok(removed_media_asset_ids)
}

async fn archive_unreferenced_assets(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    media_asset_ids: &HashSet<uuid::Uuid>,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    if media_asset_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE commerce.media_assets AS media \
         SET status='archived', archived_at=$3, updated_at=$3 \
         WHERE media.store_id=$1 AND media.id=ANY($2::uuid[]) AND media.status<>'archived' \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_media_assets AS link \
             WHERE link.store_id=media.store_id AND link.media_asset_id=media.id \
               AND link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_option_value_media_assets AS link \
             WHERE link.store_id=media.store_id AND link.media_asset_id=media.id \
               AND link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_variant_media_assets AS link \
             WHERE link.store_id=media.store_id AND link.media_asset_id=media.id \
               AND link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.review_media_assets AS link \
             WHERE link.store_id=media.store_id AND link.media_asset_id=media.id \
               AND link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_meta_media_assets AS link \
             WHERE link.store_id=media.store_id AND link.media_asset_id=media.id \
               AND link.archived_at IS NULL \
           )",
    )
    .bind(store_id.as_uuid())
    .bind(media_asset_ids.iter().copied().collect::<Vec<_>>())
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn validate_existing_ids(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    draft: &ProductConfigurationDraft,
) -> Result<(), ApplicationError> {
    let option_ids = draft
        .options
        .iter()
        .map(|option| option.id.as_uuid())
        .collect::<Vec<_>>();
    if !option_ids.is_empty() {
        let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
            "SELECT id, product_id FROM commerce.product_options \
             WHERE store_id=$1 AND id=ANY($2::uuid[])",
        )
        .bind(store_id.as_uuid())
        .bind(&option_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        ensure_ids_belong_to_product(rows, product_id, "option_id")?;
    }

    let value_ids = draft
        .options
        .iter()
        .flat_map(|option| option.values.iter().map(|value| value.id.as_uuid()))
        .collect::<Vec<_>>();
    if !value_ids.is_empty() {
        let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, uuid::Uuid)>(
            "SELECT id, product_id, option_id FROM commerce.product_option_values \
             WHERE store_id=$1 AND id=ANY($2::uuid[])",
        )
        .bind(store_id.as_uuid())
        .bind(&value_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let desired_options = draft
            .options
            .iter()
            .map(|option| {
                (
                    option.id.as_uuid(),
                    option
                        .values
                        .iter()
                        .map(|value| value.id.as_uuid())
                        .collect::<HashSet<_>>(),
                )
            })
            .collect::<Vec<_>>();
        for (id, owner_product_id, owner_option_id) in rows {
            if owner_product_id != product_id.as_uuid()
                || !desired_options.iter().any(|(option_id, values)| {
                    *option_id == owner_option_id && values.contains(&id)
                })
            {
                return Err(invalid_configuration_id("option_value_id"));
            }
        }
    }

    let variant_ids = draft
        .variants
        .iter()
        .map(|variant| variant.id.as_uuid())
        .collect::<Vec<_>>();
    if !variant_ids.is_empty() {
        let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
            "SELECT id, product_id FROM commerce.product_variants \
             WHERE store_id=$1 AND id=ANY($2::uuid[])",
        )
        .bind(store_id.as_uuid())
        .bind(&variant_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        ensure_ids_belong_to_product(rows, product_id, "variant_id")?;
    }
    Ok(())
}

fn ensure_ids_belong_to_product(
    rows: Vec<(uuid::Uuid, uuid::Uuid)>,
    product_id: ProductId,
    field: &'static str,
) -> Result<(), ApplicationError> {
    if rows.iter().any(|(_, owner)| *owner != product_id.as_uuid()) {
        return Err(invalid_configuration_id(field));
    }
    Ok(())
}

fn invalid_configuration_id(field: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: "must identify a record belonging to this Product".into(),
        }],
    }
}

async fn archive_options(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_options \
         SET archived_at=$3, updated_at=$3 \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn archive_option_values(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_option_values \
         SET archived_at=$3, updated_at=$3 \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn upsert_option(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option: &ProductConfigurationOptionInput,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_options \
         (id, store_id, product_id, name, position, archived_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,NULL,$6) \
         ON CONFLICT (store_id, product_id, id) DO UPDATE SET \
             store_id=EXCLUDED.store_id, product_id=EXCLUDED.product_id, \
             name=EXCLUDED.name, position=EXCLUDED.position, archived_at=NULL, \
             updated_at=EXCLUDED.updated_at \
         WHERE commerce.product_options.store_id=EXCLUDED.store_id \
           AND commerce.product_options.product_id=EXCLUDED.product_id",
    )
    .bind(option.id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(&option.name)
    .bind(i16::try_from(option.position).map_err(|_| invalid_configuration("option_position"))?)
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_configuration_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upsert_option_value(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    value: &ProductConfigurationOptionValueInput,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_option_values \
         (id, store_id, product_id, option_id, value, position, archived_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,NULL,$7) \
         ON CONFLICT (store_id, product_id, option_id, id) DO UPDATE SET \
             store_id=EXCLUDED.store_id, product_id=EXCLUDED.product_id, \
             option_id=EXCLUDED.option_id, value=EXCLUDED.value, position=EXCLUDED.position, \
             archived_at=NULL, updated_at=EXCLUDED.updated_at \
         WHERE commerce.product_option_values.store_id=EXCLUDED.store_id \
           AND commerce.product_option_values.product_id=EXCLUDED.product_id \
           AND commerce.product_option_values.option_id=EXCLUDED.option_id",
    )
    .bind(value.id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(option_id.as_uuid())
    .bind(&value.value)
    .bind(
        i16::try_from(value.position)
            .map_err(|_| invalid_configuration("option_value_position"))?,
    )
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_configuration_error)?;
    Ok(())
}

async fn upsert_variant(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    variant: &ProductConfigurationVariantInput,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_variants \
         (id, store_id, product_id, title, sku, status, track_inventory, meta, updated_at) \
         VALUES ($1,$2,$3,$4,$5,'active',$6,$7::jsonb,$8) \
         ON CONFLICT (store_id, product_id, id) DO UPDATE SET \
             title=EXCLUDED.title, sku=EXCLUDED.sku, status='active', \
             track_inventory=EXCLUDED.track_inventory, meta=EXCLUDED.meta, updated_at=EXCLUDED.updated_at",
    )
    .bind(variant.id.as_uuid())
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(&variant.title)
    .bind(variant.sku.as_deref())
    .bind(variant.track_inventory)
    .bind(&variant.metadata)
    .bind(changed_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_configuration_error)?;
    Ok(())
}

fn ensure_expected_revision(
    current_revision: i64,
    expected_revision: Option<i64>,
) -> Result<(), ApplicationError> {
    if expected_revision.is_some_and(|expected| expected != current_revision) {
        return Err(ApplicationError::Conflict {
            code: "product_revision_mismatch",
            message: "the Product changed; refresh the workspace and retry with its current revision",
        });
    }
    Ok(())
}

fn invalid_configuration(field: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: "does not describe a valid Product configuration".into(),
        }],
    }
}

fn map_configuration_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error {
        match database_error.constraint() {
            Some("product_options_active_name_idx") => {
                return ApplicationError::Conflict {
                    code: "product_option_name_taken",
                    message: "an active Product option already uses this name",
                };
            }
            Some("product_options_active_position_idx") => {
                return ApplicationError::Conflict {
                    code: "product_option_position_taken",
                    message: "an active Product option already uses this position",
                };
            }
            Some("product_option_values_active_value_idx") => {
                return ApplicationError::Conflict {
                    code: "product_option_value_taken",
                    message: "an active Product option already uses this value",
                };
            }
            Some("product_option_values_active_position_idx") => {
                return ApplicationError::Conflict {
                    code: "product_option_value_position_taken",
                    message: "an active Product option already uses this value position",
                };
            }
            Some("product_variants_store_sku_key") => {
                return ApplicationError::Conflict {
                    code: "product_variant_sku_taken",
                    message: "the Product variant SKU is already in use for this Store",
                };
            }
            _ => {}
        }
    }
    database_error(error)
}
