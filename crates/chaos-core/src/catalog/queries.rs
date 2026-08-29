use std::sync::Arc;

use chaos_domain::{
    catalog::{ProductId, ProductStatus},
    store::StoreId,
};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresCatalogReadRepository,
    contracts::{AdminActor, CatalogProductDetail, CatalogProductListItem},
};

pub struct ProductPage {
    pub items: Vec<CatalogProductListItem>,
    pub has_more: bool,
}

pub struct CatalogQueries {
    repository: Arc<PostgresCatalogReadRepository>,
}

impl CatalogQueries {
    pub fn new(repository: Arc<PostgresCatalogReadRepository>) -> Self {
        Self { repository }
    }

    pub async fn list_products(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductId>,
        limit: u16,
        query: Option<&str>,
        status: Option<ProductStatus>,
    ) -> Result<ProductPage, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let query = query.filter(|value| !value.trim().is_empty());
        let mut items = self
            .repository
            .list_products(actor, store_id, after, limit + 1, query, status)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "store",
                id: store_id.as_uuid().to_string(),
            })?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(ProductPage { items, has_more })
    }

    pub async fn get_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<CatalogProductDetail, ApplicationError> {
        self.repository
            .get_product(actor, store_id, product_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "product",
                id: product_id.as_uuid().to_string(),
            })
    }
}
