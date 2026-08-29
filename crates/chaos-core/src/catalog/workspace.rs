use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    catalog::{MediaAssetStatus, ProductId, ProductVariantId, VariantStatus},
    store::StoreId,
};

use crate::{
    ApplicationError,
    adapters::postgres::{PostgresCatalogReadRepository, PostgresMediaAssetRepository},
    contracts::{AdminActor, CatalogProductWorkspace, ProductMediaAssetItem, ProductMediaScope},
};

pub struct ProductWorkspaceQueries {
    catalog_repository: Arc<PostgresCatalogReadRepository>,
    media_repository: Arc<PostgresMediaAssetRepository>,
}

impl ProductWorkspaceQueries {
    pub fn new(
        catalog_repository: Arc<PostgresCatalogReadRepository>,
        media_repository: Arc<PostgresMediaAssetRepository>,
    ) -> Self {
        Self {
            catalog_repository,
            media_repository,
        }
    }

    pub async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<CatalogProductWorkspace, ApplicationError> {
        for _attempt in 0..2 {
            let product = self
                .catalog_repository
                .get_product(actor.clone(), store_id, product_id)
                .await?
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "product",
                    id: product_id.as_uuid().to_string(),
                })?;
            let media = self
                .media_repository
                .list_product(actor.clone(), store_id, product_id)
                .await?
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "product",
                    id: product_id.as_uuid().to_string(),
                })?;
            let publications = self
                .catalog_repository
                .list_product_publications(actor.clone(), store_id, product_id)
                .await?;
            let current_revision = self
                .catalog_repository
                .product_revision(actor.clone(), store_id, product_id)
                .await?
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "product",
                    id: product_id.as_uuid().to_string(),
                })?;
            if current_revision == product.revision {
                return Ok(CatalogProductWorkspace {
                    product,
                    media,
                    publications,
                });
            }
        }
        Err(ApplicationError::Conflict {
            code: "product_changed_during_read",
            message: "the Product changed while its workspace was being read; retry the workspace read",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductMediaResolutionSource {
    None,
    Product,
    OptionValue,
    Variant,
}

impl ProductMediaResolutionSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Product => "product",
            Self::OptionValue => "option_value",
            Self::Variant => "variant",
        }
    }
}

pub struct ResolvedProductMedia {
    pub source: ProductMediaResolutionSource,
    pub matched_option_value_ids: Vec<chaos_domain::catalog::ProductOptionValueId>,
    pub items: Vec<ProductMediaAssetItem>,
}

pub fn resolve_product_media(
    workspace: &CatalogProductWorkspace,
    product_variant_id: ProductVariantId,
) -> Result<ResolvedProductMedia, ApplicationError> {
    let variant = workspace
        .product
        .variants
        .iter()
        .find(|variant| variant.id == product_variant_id)
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "product_variant",
            id: product_variant_id.as_uuid().to_string(),
        })?;
    if variant.status != VariantStatus::Active {
        return Err(ApplicationError::Conflict {
            code: "product_variant_archived",
            message: "effective storefront media can only be resolved for an active Variant",
        });
    }
    let selected_option_pairs = variant
        .selected_options
        .iter()
        .map(|selection| (selection.option_id, selection.option_value_id))
        .collect::<HashSet<_>>();
    let usable = |item: &&ProductMediaAssetItem| {
        item.archived_at.is_none()
            && item.asset.status == MediaAssetStatus::Ready
            && item.asset.public_url.is_some()
    };
    let mut variant_items = workspace
        .media
        .iter()
        .filter(|item| {
            usable(item)
                && item.product_id == workspace.product.id
                && matches!(
                    item.scope,
                    ProductMediaScope::Variant {
                        product_variant_id: id
                    } if id == product_variant_id
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !variant_items.is_empty() {
        sort_media(&mut variant_items);
        return Ok(ResolvedProductMedia {
            source: ProductMediaResolutionSource::Variant,
            matched_option_value_ids: Vec::new(),
            items: variant_items,
        });
    }

    let mut option_items = workspace
        .media
        .iter()
        .filter(|item| {
            usable(item)
                && matches!(
                    item.scope,
                    ProductMediaScope::OptionValue {
                        option_id,
                        option_value_id,
                    } if selected_option_pairs.contains(&(option_id, option_value_id))
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    deduplicate_media(&mut option_items);
    if !option_items.is_empty() {
        sort_media(&mut option_items);
        return Ok(ResolvedProductMedia {
            source: ProductMediaResolutionSource::OptionValue,
            matched_option_value_ids: variant
                .selected_options
                .iter()
                .filter_map(|selection| {
                    option_items
                        .iter()
                        .any(|item| {
                            matches!(
                                item.scope,
                                ProductMediaScope::OptionValue {
                                    option_id,
                                    option_value_id,
                                } if option_id == selection.option_id
                                    && option_value_id == selection.option_value_id
                            )
                        })
                        .then_some(selection.option_value_id)
                })
                .collect(),
            items: option_items,
        });
    }

    let mut product_items = workspace
        .media
        .iter()
        .filter(|item| usable(item) && matches!(item.scope, ProductMediaScope::Product))
        .cloned()
        .collect::<Vec<_>>();
    deduplicate_media(&mut product_items);
    sort_media(&mut product_items);
    Ok(ResolvedProductMedia {
        source: if product_items.is_empty() {
            ProductMediaResolutionSource::None
        } else {
            ProductMediaResolutionSource::Product
        },
        matched_option_value_ids: Vec::new(),
        items: product_items,
    })
}

fn deduplicate_media(items: &mut Vec<ProductMediaAssetItem>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.asset.id));
}

fn sort_media(items: &mut [ProductMediaAssetItem]) {
    items.sort_by_key(|item| (item.position, item.asset.id.as_uuid()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        CatalogProductDetail, CatalogProductVariant, CatalogSelectedOption, MediaAssetItem,
    };
    use chaos_domain::{
        catalog::{
            MediaAssetId, MediaAssetStatus, MediaKind, ProductOptionId, ProductOptionValueId,
            ProductStatus, VariantStatus,
        },
        store::StoreId,
    };
    use time::OffsetDateTime;

    #[test]
    fn resolves_variant_then_option_value_then_product_media() {
        let store_id = StoreId::new();
        let product_id = ProductId::new();
        let option_id = ProductOptionId::new();
        let value_id = ProductOptionValueId::new();
        let variant_id = ProductVariantId::new();
        let now = OffsetDateTime::now_utc();
        let asset = |id| MediaAssetItem {
            id,
            store_id,
            file_name: "image.webp".into(),
            media_type: "image/webp".into(),
            kind: MediaKind::Image,
            byte_size: 1,
            sha256_hex: "a".repeat(64),
            status: MediaAssetStatus::Ready,
            public_url: Some("https://cdn.example/image.webp".into()),
            created_at: now,
            updated_at: now,
        };
        let link = |asset, scope| ProductMediaAssetItem {
            asset,
            product_id,
            scope,
            alt_text: String::new(),
            position: 0,
            archived_at: None,
        };
        let workspace = |media| CatalogProductWorkspace {
            product: CatalogProductDetail {
                id: product_id,
                handle: "shirt".into(),
                title: "Shirt".into(),
                description: String::new(),
                status: ProductStatus::Active,
                revision: 1,
                options: Vec::new(),
                variants: vec![CatalogProductVariant {
                    id: variant_id,
                    title: "Blue".into(),
                    sku: None,
                    status: VariantStatus::Active,
                    track_inventory: true,
                    selected_options: vec![CatalogSelectedOption {
                        option_id,
                        option_name: "Color".into(),
                        option_value_id: value_id,
                        value: "Blue".into(),
                    }],
                    metadata: None,
                    created_at: now,
                    updated_at: now,
                }],
                metadata: None,
                created_at: now,
                updated_at: now,
            },
            media,
            publications: Vec::new(),
        };
        let product_asset = MediaAssetId::new();
        let option_asset = MediaAssetId::new();
        let variant_asset = MediaAssetId::new();
        let product = workspace(vec![link(asset(product_asset), ProductMediaScope::Product)]);
        assert_eq!(
            resolve_product_media(&product, variant_id).unwrap().source,
            ProductMediaResolutionSource::Product
        );
        let option = workspace(vec![link(
            asset(option_asset),
            ProductMediaScope::OptionValue {
                option_id,
                option_value_id: value_id,
            },
        )]);
        assert_eq!(
            resolve_product_media(&option, variant_id).unwrap().source,
            ProductMediaResolutionSource::OptionValue
        );
        let variant = workspace(vec![link(
            asset(variant_asset),
            ProductMediaScope::Variant {
                product_variant_id: variant_id,
            },
        )]);
        assert_eq!(
            resolve_product_media(&variant, variant_id).unwrap().source,
            ProductMediaResolutionSource::Variant
        );
    }
}
