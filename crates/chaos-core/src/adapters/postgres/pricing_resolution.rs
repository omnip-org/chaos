use crate::{ApplicationError, error::database_error};
use chaos_domain::{FieldViolation, catalog::ProductId, store::StoreId};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct PriceCoverageIssue {
    pub(crate) checked_at: OffsetDateTime,
    pub(crate) price_list_id: Option<Uuid>,
    pub(crate) price_list_name: Option<String>,
    pub(crate) product_id: ProductId,
    pub(crate) product_handle: String,
    pub(crate) variant_skus: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PriceCoverageSnapshot {
    pub(crate) checkpoints: Vec<OffsetDateTime>,
    pub(crate) issues: Vec<PriceCoverageIssue>,
}

pub(crate) async fn lock_store_price_context(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock( \
             hashtextextended('chaos-price-context:' || $1::text, 0) \
         )",
    )
    .bind(store_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

pub(crate) async fn price_coverage_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    additional_checkpoints: &[OffsetDateTime],
) -> Result<PriceCoverageSnapshot, ApplicationError> {
    let checkpoints = sqlx::query_scalar::<_, OffsetDateTime>(
        "WITH requested_points AS ( \
             SELECT unnest($2::timestamptz[]) AS checked_at \
         ), evaluation_points AS ( \
             SELECT CURRENT_TIMESTAMP AS checked_at \
             UNION \
             SELECT price_list.starts_at \
             FROM chaos_commerce.price_lists AS price_list \
             WHERE price_list.store_id = $1 \
               AND price_list.starts_at > CURRENT_TIMESTAMP \
             UNION \
             SELECT price_list.ends_at \
             FROM chaos_commerce.price_lists AS price_list \
             WHERE price_list.store_id = $1 \
               AND price_list.ends_at > CURRENT_TIMESTAMP \
             UNION \
             SELECT requested.checked_at \
             FROM requested_points AS requested \
             WHERE requested.checked_at > CURRENT_TIMESTAMP \
         ) \
         SELECT checked_at FROM evaluation_points ORDER BY checked_at",
    )
    .bind(store_id.as_uuid())
    .bind(additional_checkpoints.to_vec())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let issues = price_coverage_issues_at(transaction, store_id, &checkpoints).await?;
    Ok(PriceCoverageSnapshot {
        checkpoints,
        issues,
    })
}

pub(crate) async fn price_coverage_issues_at(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    checkpoints: &[OffsetDateTime],
) -> Result<Vec<PriceCoverageIssue>, ApplicationError> {
    let rows = sqlx::query_as::<
        _,
        (
            OffsetDateTime,
            Option<Uuid>,
            Option<String>,
            Uuid,
            String,
            Vec<String>,
        ),
    >(
        "WITH store_context AS ( \
             SELECT id, currency \
             FROM chaos_commerce.stores \
             WHERE id = $1 AND status = 'active' \
         ), evaluation_points AS ( \
             SELECT unnest($2::timestamptz[]) AS checked_at \
         ), selected_price_lists AS ( \
             SELECT point.checked_at, selected.id AS price_list_id, \
                    price_list.name AS price_list_name \
             FROM evaluation_points AS point \
             CROSS JOIN store_context AS store \
             LEFT JOIN LATERAL chaos_commerce.resolve_price_list( \
                 store.id, store.currency, point.checked_at \
             ) AS selected ON true \
             LEFT JOIN chaos_commerce.price_lists AS price_list \
               ON price_list.store_id = store.id AND price_list.id = selected.id \
         ) \
         SELECT selected.checked_at, selected.price_list_id, selected.price_list_name, \
                product.id, product.handle::text, \
                array_agg(DISTINCT COALESCE(variant.sku::text, '(no SKU)')) \
                    FILTER (WHERE price.product_variant_id IS NULL) AS unpriced_variant_skus \
         FROM selected_price_lists AS selected \
         INNER JOIN chaos_commerce.products AS product \
           ON product.store_id = $1 AND product.status = 'active' \
         INNER JOIN chaos_commerce.product_variants AS variant \
           ON variant.store_id = product.store_id AND variant.product_id = product.id \
          AND variant.status = 'active' \
         LEFT JOIN chaos_commerce.price_list_items AS price \
           ON price.store_id = variant.store_id \
          AND price.price_list_id = selected.price_list_id \
          AND price.product_variant_id = variant.id \
         WHERE EXISTS ( \
             SELECT 1 \
             FROM chaos_commerce.product_publications AS publication \
             INNER JOIN chaos_commerce.channels AS channel \
               ON channel.store_id = publication.store_id \
              AND channel.id = publication.channel_id \
              AND channel.status = 'active' \
             WHERE publication.store_id = product.store_id \
               AND publication.product_id = product.id \
         ) \
         GROUP BY selected.checked_at, selected.price_list_id, selected.price_list_name, \
                  product.id, product.handle \
         HAVING count(price.product_variant_id) = 0 \
         ORDER BY selected.checked_at, product.handle",
    )
    .bind(store_id.as_uuid())
    .bind(checkpoints.to_vec())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;

    Ok(rows
        .into_iter()
        .map(
            |(
                checked_at,
                price_list_id,
                price_list_name,
                product_id,
                product_handle,
                variant_skus,
            )| {
                PriceCoverageIssue {
                    checked_at,
                    price_list_id,
                    price_list_name,
                    product_id: ProductId::from_uuid(product_id),
                    product_handle,
                    variant_skus,
                }
            },
        )
        .collect())
}

pub(crate) fn reject_new_price_coverage_issues(
    before: &PriceCoverageSnapshot,
    after: &[PriceCoverageIssue],
) -> Result<(), ApplicationError> {
    let existing = before
        .issues
        .iter()
        .map(|issue| {
            (
                issue.checked_at.unix_timestamp_nanos(),
                issue.price_list_id,
                issue.product_id,
            )
        })
        .collect::<std::collections::HashSet<_>>();
    let mut reported = std::collections::HashSet::new();
    let introduced = after
        .iter()
        .filter(|issue| {
            let checkpoint_key = (
                issue.checked_at.unix_timestamp_nanos(),
                issue.price_list_id,
                issue.product_id,
            );
            !existing.contains(&checkpoint_key)
                && reported.insert((issue.price_list_id, issue.product_id))
        })
        .cloned()
        .collect::<Vec<_>>();
    reject_any_price_coverage_issues(&introduced)
}

pub(crate) fn reject_any_price_coverage_issues(
    issues: &[PriceCoverageIssue],
) -> Result<(), ApplicationError> {
    let mut reported = std::collections::HashSet::new();
    let unique = issues
        .iter()
        .filter(|issue| reported.insert((issue.price_list_id, issue.product_id)))
        .cloned()
        .collect::<Vec<_>>();
    if unique.is_empty() {
        Ok(())
    } else {
        Err(price_coverage_error(&unique))
    }
}

pub(crate) async fn ensure_product_is_priced(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    channel_id: Option<Uuid>,
) -> Result<(), ApplicationError> {
    let issue = sqlx::query_as::<
        _,
        (
            OffsetDateTime,
            String,
            Option<Uuid>,
            Option<String>,
            Vec<String>,
        ),
    >(
        "WITH store_context AS ( \
             SELECT id, currency \
             FROM chaos_commerce.stores \
             WHERE id = $1 AND status = 'active' \
         ), evaluation_points AS ( \
             SELECT CURRENT_TIMESTAMP AS checked_at \
             UNION \
             SELECT price_list.starts_at \
             FROM chaos_commerce.price_lists AS price_list \
             INNER JOIN store_context AS store ON store.id = price_list.store_id \
             WHERE price_list.status = 'active' \
               AND price_list.currency = store.currency \
               AND price_list.starts_at > CURRENT_TIMESTAMP \
             UNION \
             SELECT price_list.ends_at \
             FROM chaos_commerce.price_lists AS price_list \
             INNER JOIN store_context AS store ON store.id = price_list.store_id \
             WHERE price_list.status = 'active' \
               AND price_list.currency = store.currency \
               AND price_list.ends_at > CURRENT_TIMESTAMP \
         ), selected_price_lists AS ( \
             SELECT point.checked_at, selected.id, price_list.name \
             FROM evaluation_points AS point \
             CROSS JOIN store_context AS store \
             LEFT JOIN LATERAL chaos_commerce.resolve_price_list( \
                 store.id, store.currency, point.checked_at \
             ) AS selected ON true \
             LEFT JOIN chaos_commerce.price_lists AS price_list \
               ON price_list.store_id = store.id AND price_list.id = selected.id \
         ) \
         SELECT selected.checked_at, product.handle::text, selected.id, selected.name, \
                array_agg(DISTINCT COALESCE(variant.sku::text, '(no SKU)')) \
                    FILTER (WHERE price.product_variant_id IS NULL) AS unpriced_variant_skus \
         FROM chaos_commerce.products AS product \
         CROSS JOIN selected_price_lists AS selected \
         INNER JOIN chaos_commerce.product_variants AS variant \
           ON variant.store_id = product.store_id AND variant.product_id = product.id \
          AND variant.status = 'active' \
         LEFT JOIN chaos_commerce.price_list_items AS price \
           ON price.store_id = variant.store_id \
          AND price.price_list_id = selected.id \
          AND price.product_variant_id = variant.id \
         WHERE product.store_id = $1 AND product.id = $2 AND product.status = 'active' \
           AND ( \
             $3::uuid IS NOT NULL \
             OR EXISTS ( \
                 SELECT 1 \
                 FROM chaos_commerce.product_publications AS publication \
                 INNER JOIN chaos_commerce.channels AS channel \
                   ON channel.store_id = publication.store_id \
                  AND channel.id = publication.channel_id \
                  AND channel.status = 'active' \
                 WHERE publication.store_id = product.store_id \
                   AND publication.product_id = product.id \
             ) \
           ) \
         GROUP BY selected.checked_at, product.handle, selected.id, selected.name \
         HAVING count(price.product_variant_id) = 0 \
         ORDER BY selected.checked_at LIMIT 1",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(channel_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    if let Some((checked_at, product_handle, price_list_id, price_list_name, variant_skus)) = issue
    {
        return Err(price_coverage_error(&[PriceCoverageIssue {
            checked_at,
            price_list_id,
            price_list_name,
            product_id,
            product_handle,
            variant_skus,
        }]));
    }
    Ok(())
}

fn price_coverage_error(issues: &[PriceCoverageIssue]) -> ApplicationError {
    const MAX_DETAILS: usize = 20;
    let mut details = issues
        .iter()
        .take(MAX_DETAILS)
        .map(|issue| {
            let price_list = match (&issue.price_list_name, issue.price_list_id) {
                (Some(name), Some(id)) => format!("{name} ({id})"),
                _ => "no effective price list".to_owned(),
            };
            format!(
                "{} at {}: product '{}' has no priced active variant (SKUs: {})",
                price_list,
                issue.checked_at,
                issue.product_handle,
                issue.variant_skus.join(", ")
            )
        })
        .collect::<Vec<_>>();
    if issues.len() > MAX_DETAILS {
        details.push(format!(
            "and {} more affected products",
            issues.len() - MAX_DETAILS
        ));
    }
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "prices",
            reason: details.join("; "),
        }],
    }
}
