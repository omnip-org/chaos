use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, Locale,
    catalog::{ProductId, ProductOptionId, ProductOptionValueId, ProductVariantId},
    merchant::{ApiKeyMode, MerchantAccountId, SalesChannelId, StoreId},
};

use crate::{ApplicationError, ports::MachineActor};

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
pub struct StorefrontSelectedOption {
    pub option_id: ProductOptionId,
    pub option_value_id: ProductOptionValueId,
}

pub struct StorefrontCatalogVariant {
    pub id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub tax_inclusive: bool,
    pub selected_options: Vec<StorefrontSelectedOption>,
    pub metadata: Option<serde_json::Value>,
}

pub struct StorefrontCatalogProduct {
    pub id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub locale: Locale,
    pub options: Vec<StorefrontProductOption>,
    pub variants: Vec<StorefrontCatalogVariant>,
    pub media: Vec<StorefrontMediaAsset>,
    pub metadata: Option<serde_json::Value>,
}

pub struct StorefrontMediaAsset {
    pub id: chaos_domain::catalog::MediaAssetId,
    pub product_variant_id: Option<ProductVariantId>,
    pub media_type: String,
    pub kind: chaos_domain::catalog::MediaKind,
    pub alt_text: String,
    pub position: u16,
    pub url: String,
}

#[async_trait]
pub trait StorefrontCatalogRepository: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn list_products(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        locale: Option<Locale>,
        query: Option<&str>,
        collection_handle: Option<&str>,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCatalogProduct>, ApplicationError>;

    async fn get_product_by_handle(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        locale: Option<Locale>,
        handle: &str,
    ) -> Result<Option<StorefrontCatalogProduct>, ApplicationError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorefrontContext {
    pub merchant_account_id: MerchantAccountId,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub key_mode: ApiKeyMode,
}
