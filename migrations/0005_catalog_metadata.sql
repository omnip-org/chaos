ALTER TABLE catalog.products ADD COLUMN metadata JSONB;
ALTER TABLE catalog.products ADD CONSTRAINT products_metadata_size_check CHECK (
    metadata IS NULL OR octet_length(metadata::text) <= 32768
);

ALTER TABLE catalog.product_variants ADD COLUMN metadata JSONB;
ALTER TABLE catalog.product_variants ADD CONSTRAINT product_variants_metadata_size_check CHECK (
    metadata IS NULL OR octet_length(metadata::text) <= 32768
);

ALTER TABLE catalog.collections ADD COLUMN metadata JSONB;
ALTER TABLE catalog.collections ADD CONSTRAINT collections_metadata_size_check CHECK (
    metadata IS NULL OR octet_length(metadata::text) <= 32768
);
