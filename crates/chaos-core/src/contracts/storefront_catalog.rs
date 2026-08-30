use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::{CollectionId, ProductId, ProductOptionId, ProductOptionValueId, ProductVariantId},
    store::{SalesChannelId, StoreId},
};
use std::collections::HashSet;

use crate::{ApplicationError, contracts::MachineActor};

pub struct StorefrontProductOptionValue {
    pub id: ProductOptionValueId,
    pub value: String,
    pub position: u16,
}

pub struct StorefrontProductOption {
    pub id: ProductOptionId,
    pub name: String,
    pub position: u16,
    pub values: Vec<StorefrontProductOptionValue>,
}

/// A Variant's value for one Product Option — e.g. `{ option: "Color", value: "Forest" }` —
/// so a Storefront client can resolve the exact Variant matching a customer's full
/// selection without re-deriving it from `title`, which carries no stable structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorefrontSelectedOption {
    pub option_id: ProductOptionId,
    pub option_value_id: ProductOptionValueId,
}

pub struct StorefrontCatalogVariant {
    pub id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub track_inventory: bool,
    pub available_quantity: i64,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub selected_options: Vec<StorefrontSelectedOption>,
    pub metadata: Option<serde_json::Value>,
}

/// A Collection this Product is a member of, published to the Store's
/// current Sales Channel — enough for a Storefront client to link back to
/// the parent Collection (breadcrumb, "shop this collection") without a
/// second round trip. A Product may belong to more than one Collection;
/// order is by handle, since collection_products.position is meaningful only
/// within one Collection's own product listing, not across Collections.
pub struct StorefrontProductCollection {
    pub id: CollectionId,
    pub handle: String,
    pub title: String,
}

pub struct StorefrontCatalogProduct {
    pub id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub options: Vec<StorefrontProductOption>,
    pub variants: Vec<StorefrontCatalogVariant>,
    pub media: Vec<StorefrontMediaAsset>,
    pub collections: Vec<StorefrontProductCollection>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct StorefrontMediaAsset {
    pub id: chaos_domain::catalog::MediaAssetId,
    pub scope: StorefrontMediaScope,
    pub media_type: String,
    pub kind: chaos_domain::catalog::MediaKind,
    pub alt_text: String,
    pub position: u16,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorefrontMediaScope {
    Product,
    OptionValue {
        option_id: ProductOptionId,
        option_value_id: ProductOptionValueId,
    },
    Variant {
        product_variant_id: ProductVariantId,
    },
}

/// Resolves the media that should be shown for one selected Variant.
/// Exact Variant media overrides Option Value media, which overrides Product media.
/// Repeated links to the same physical asset are returned only once.
pub fn resolve_storefront_media(
    media: &[StorefrontMediaAsset],
    variant_id: ProductVariantId,
    selected_options: &[StorefrontSelectedOption],
) -> Vec<StorefrontMediaAsset> {
    let exact = media
        .iter()
        .filter(|asset| {
            matches!(
                asset.scope,
                StorefrontMediaScope::Variant {
                    product_variant_id: id
                } if id == variant_id
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return deduplicate_media(exact);
    }

    let selected_options = selected_options
        .iter()
        .map(|selection| {
            (
                selection.option_id.as_uuid(),
                selection.option_value_id.as_uuid(),
            )
        })
        .collect::<HashSet<_>>();
    let option_value_media = media
        .iter()
        .filter(|asset| {
            matches!(
                asset.scope,
                StorefrontMediaScope::OptionValue {
                    option_id,
                    option_value_id,
                } if selected_options.contains(&(option_id.as_uuid(), option_value_id.as_uuid()))
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !option_value_media.is_empty() {
        return deduplicate_media(option_value_media);
    }

    deduplicate_media(
        media
            .iter()
            .filter(|asset| matches!(asset.scope, StorefrontMediaScope::Product))
            .cloned()
            .collect(),
    )
}

fn deduplicate_media(mut media: Vec<StorefrontMediaAsset>) -> Vec<StorefrontMediaAsset> {
    let mut seen = HashSet::with_capacity(media.len());
    media.retain(|asset| seen.insert(asset.id.as_uuid()));
    media.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.id.as_uuid().cmp(&right.id.as_uuid()))
    });
    media
}

#[async_trait]
pub trait StorefrontCatalogRepository: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn list_products(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        query: Option<&str>,
        collection_handle: Option<&str>,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCatalogProduct>, ApplicationError>;

    async fn get_product_by_handle(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        handle: &str,
    ) -> Result<Option<StorefrontCatalogProduct>, ApplicationError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorefrontContext {
    pub store_id: StoreId,
    pub channel_id: SalesChannelId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::catalog::{MediaAssetId, MediaKind};

    fn media(id: &str, scope: StorefrontMediaScope, position: u16) -> StorefrontMediaAsset {
        StorefrontMediaAsset {
            id: MediaAssetId::from_uuid(id.parse().unwrap()),
            scope,
            media_type: "image/jpeg".into(),
            kind: MediaKind::Image,
            alt_text: String::new(),
            position,
            url: format!("https://cdn.example/{id}.jpg"),
        }
    }

    #[test]
    fn exact_variant_media_overrides_option_value_and_product_media() {
        let variant_id =
            ProductVariantId::from_uuid("00000000-0000-0000-0000-000000000001".parse().unwrap());
        let media = vec![
            media(
                "00000000-0000-0000-0000-000000000010",
                StorefrontMediaScope::Product,
                0,
            ),
            media(
                "00000000-0000-0000-0000-000000000011",
                StorefrontMediaScope::OptionValue {
                    option_id: ProductOptionId::from_uuid(
                        "00000000-0000-0000-0000-000000000020".parse().unwrap(),
                    ),
                    option_value_id: ProductOptionValueId::from_uuid(
                        "00000000-0000-0000-0000-000000000021".parse().unwrap(),
                    ),
                },
                0,
            ),
            media(
                "00000000-0000-0000-0000-000000000012",
                StorefrontMediaScope::Variant {
                    product_variant_id: variant_id,
                },
                0,
            ),
        ];
        let resolved = resolve_storefront_media(
            &media,
            variant_id,
            &[StorefrontSelectedOption {
                option_id: ProductOptionId::from_uuid(
                    "00000000-0000-0000-0000-000000000020".parse().unwrap(),
                ),
                option_value_id: ProductOptionValueId::from_uuid(
                    "00000000-0000-0000-0000-000000000021".parse().unwrap(),
                ),
            }],
        );

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].id.as_uuid().to_string(),
            "00000000-0000-0000-0000-000000000012"
        );
    }

    #[test]
    fn matching_option_value_media_is_deduplicated_before_product_fallback() {
        let variant_id =
            ProductVariantId::from_uuid("00000000-0000-0000-0000-000000000001".parse().unwrap());
        let option_id =
            ProductOptionId::from_uuid("00000000-0000-0000-0000-000000000020".parse().unwrap());
        let value_id = ProductOptionValueId::from_uuid(
            "00000000-0000-0000-0000-000000000021".parse().unwrap(),
        );
        let media_id = "00000000-0000-0000-0000-000000000011";
        let media = vec![
            media(
                media_id,
                StorefrontMediaScope::OptionValue {
                    option_id,
                    option_value_id: value_id,
                },
                1,
            ),
            media(
                media_id,
                StorefrontMediaScope::OptionValue {
                    option_id,
                    option_value_id: ProductOptionValueId::from_uuid(
                        "00000000-0000-0000-0000-000000000022".parse().unwrap(),
                    ),
                },
                0,
            ),
            media(
                "00000000-0000-0000-0000-000000000010",
                StorefrontMediaScope::Product,
                0,
            ),
        ];
        let resolved = resolve_storefront_media(
            &media,
            variant_id,
            &[StorefrontSelectedOption {
                option_id,
                option_value_id: value_id,
            }],
        );

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id.as_uuid().to_string(), media_id);
    }

    #[test]
    fn product_media_is_used_when_no_specific_rule_matches() {
        let variant_id =
            ProductVariantId::from_uuid("00000000-0000-0000-0000-000000000001".parse().unwrap());
        let media = vec![media(
            "00000000-0000-0000-0000-000000000010",
            StorefrontMediaScope::Product,
            0,
        )];
        let resolved = resolve_storefront_media(&media, variant_id, &[]);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].scope, StorefrontMediaScope::Product);
    }
}
