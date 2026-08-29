use chaos_domain::catalog::ProductStatus;

pub struct ProductLifecycleSnapshot {
    pub status: ProductStatus,
    pub variant_count: u32,
    pub revision: i64,
}
