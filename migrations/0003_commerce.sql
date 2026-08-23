CREATE SCHEMA commerce;

COMMENT ON SCHEMA commerce IS
    'Store-owned commerce data and Storefront read models';

CREATE TYPE commerce.store_role AS ENUM ('owner', 'member');

CREATE TYPE commerce.store_status AS ENUM ('active', 'inactive');

CREATE TYPE commerce.sales_channel_status AS ENUM ('active', 'archived');

CREATE TABLE commerce.stores (
    id                   UUID                     NOT NULL PRIMARY KEY,
    code                 extensions.citext        NOT NULL UNIQUE,
    name                 TEXT                     NOT NULL,
    default_region       CHAR(2)                  NOT NULL DEFAULT 'US',
    default_currency     CHAR(3)                  NOT NULL DEFAULT 'USD',
    default_locale       VARCHAR(63)              NOT NULL DEFAULT 'en-US',
    status               commerce.store_status    NOT NULL DEFAULT 'active',
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT stores_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT stores_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT stores_region_format_check CHECK (
        default_region ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT stores_currency_format_check CHECK (
        default_currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT stores_default_locale_check CHECK (
        default_locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE commerce.store_memberships (
    store_id    UUID                    NOT NULL,
    user_id     UUID                    NOT NULL,
    role        commerce.store_role     NOT NULL,
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, user_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE
);

CREATE TABLE commerce.store_locales (
    store_id            UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    created_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, locale),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT store_locales_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE commerce.store_sales_channels (
    id                   UUID                              NOT NULL PRIMARY KEY,
    store_id             UUID                              NOT NULL,
    code                 extensions.citext                 NOT NULL,
    name                 TEXT                              NOT NULL,
    status               commerce.sales_channel_status     NOT NULL DEFAULT 'active',
    is_default           BOOLEAN                           NOT NULL DEFAULT false,
    created_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT store_sales_channels_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT store_sales_channels_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE commerce.store_currencies (
    store_id             UUID       NOT NULL,
    currency             CHAR(3)    NOT NULL,
    enabled              BOOLEAN    NOT NULL DEFAULT true,

    PRIMARY KEY (store_id, currency),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT store_currencies_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    )
);

CREATE TABLE commerce.store_publishable_keys (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    sales_channel_id     UUID,
    public_key           TEXT                      NOT NULL UNIQUE,
    name                 TEXT                      NOT NULL,
    created_by_user_id   UUID                      NOT NULL,
    revoked_by_user_id   UUID,
    revoked_at           TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id),
    FOREIGN KEY (created_by_user_id)
        REFERENCES identity.users(id),
    FOREIGN KEY (revoked_by_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT store_publishable_keys_public_key_format_check CHECK (
        public_key ~ '^pk_[1-9A-HJ-NP-Za-km-z]{24}$'
    ),
    CONSTRAINT store_publishable_keys_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    ),
    CONSTRAINT store_publishable_keys_revocation_check CHECK (
        (revoked_at IS NULL AND revoked_by_user_id IS NULL)
        OR (revoked_at IS NOT NULL AND revoked_by_user_id IS NOT NULL)
    )
);

CREATE INDEX store_memberships_user_idx
    ON commerce.store_memberships (user_id, store_id);

CREATE INDEX stores_status_idx
    ON commerce.stores (status);

CREATE UNIQUE INDEX store_sales_channels_one_default_per_store_idx
    ON commerce.store_sales_channels (store_id)
    WHERE is_default;

CREATE INDEX store_sales_channels_store_status_idx
    ON commerce.store_sales_channels (store_id, status);

CREATE INDEX store_publishable_keys_store_created_idx
    ON commerce.store_publishable_keys (store_id, created_at DESC, id DESC);

CREATE FUNCTION commerce.prevent_default_locale_removal()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM commerce.stores
        WHERE id = OLD.store_id
          AND default_locale = OLD.locale
    ) THEN
        RAISE EXCEPTION 'the default Store Locale cannot be disabled'
            USING ERRCODE = '23514';
    END IF;
    RETURN OLD;
END
$$;

CREATE FUNCTION commerce.authenticate_publishable_key(presented_public_key TEXT)
RETURNS TABLE (
    publishable_key_id   UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    created_by_user_id   UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT publishable_key.id,
           publishable_key.store_id,
           sales_channel.id,
           publishable_key.created_by_user_id
    FROM commerce.store_publishable_keys AS publishable_key
    INNER JOIN commerce.stores AS store
        ON store.id = publishable_key.store_id
    LEFT JOIN commerce.store_sales_channels AS sales_channel
        ON sales_channel.store_id = publishable_key.store_id
       AND sales_channel.status = 'active'
       AND sales_channel.id = COALESCE(
           publishable_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM commerce.store_sales_channels AS default_channel
               WHERE default_channel.store_id = publishable_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE publishable_key.public_key = presented_public_key
      AND publishable_key.revoked_at IS NULL
      AND store.status = 'active';
$$;

CREATE TRIGGER store_locales_protect_default
BEFORE DELETE ON commerce.store_locales
FOR EACH ROW EXECUTE FUNCTION commerce.prevent_default_locale_removal();

ALTER TABLE commerce.stores ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_memberships ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_locales ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_currencies ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_sales_channels ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_publishable_keys ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.stores
    USING (id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_directory ON commerce.stores
    FOR SELECT
    USING (
        EXISTS (
            SELECT 1
            FROM commerce.store_memberships AS membership
            WHERE membership.store_id = stores.id
              AND membership.user_id =
                    nullif(current_setting('app.user_id', true), '')::uuid
        )
    );

CREATE POLICY store_isolation ON commerce.store_memberships
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_membership_directory ON commerce.store_memberships
    FOR SELECT
    USING (
        user_id = nullif(current_setting('app.user_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.store_locales
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.store_currencies
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.store_sales_channels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.store_publishable_keys
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION commerce.authenticate_publishable_key(TEXT) FROM PUBLIC;

COMMENT ON FUNCTION commerce.authenticate_publishable_key(TEXT) IS
    'Authenticates a public Storefront key';

GRANT EXECUTE
    ON FUNCTION commerce.authenticate_publishable_key(TEXT) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Catalog ===

CREATE TYPE commerce.product_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE commerce.variant_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.collection_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE commerce.media_kind AS ENUM ('image', 'video');

CREATE TYPE commerce.media_asset_status AS ENUM ('pending_upload', 'ready', 'archived');

CREATE TYPE commerce.review_status AS ENUM ('pending', 'approved', 'rejected');

CREATE TABLE commerce.products (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               commerce.product_status    NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT products_handle_format_check CHECK (
        handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'
    ),
    CONSTRAINT products_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT products_description_length_check CHECK (
        length(description) <= 100000
    ),
    CONSTRAINT products_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.product_options (
    id                   UUID                 NOT NULL PRIMARY KEY,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    name                 extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, product_id, name),
    UNIQUE (store_id, product_id, position),
    UNIQUE (store_id, product_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_options_name_length_check CHECK (
        length(trim(name::text)) BETWEEN 1 AND 80
    ),
    CONSTRAINT product_options_position_check CHECK (
        position BETWEEN 0 AND 9
    )
);

CREATE TABLE commerce.product_option_values (
    id                   UUID                 NOT NULL PRIMARY KEY,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    option_id            UUID                 NOT NULL,
    value                extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, product_id, option_id, value),
    UNIQUE (store_id, product_id, option_id, position),
    UNIQUE (store_id, product_id, option_id, id),
    FOREIGN KEY (store_id, product_id, option_id)
        REFERENCES commerce.product_options(store_id, product_id, id)
        ON DELETE CASCADE,
    CONSTRAINT product_option_values_value_length_check CHECK (
        length(trim(value::text)) BETWEEN 1 AND 120
    ),
    CONSTRAINT product_option_values_position_check CHECK (
        position BETWEEN 0 AND 999
    )
);

CREATE TABLE commerce.product_variants (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    product_id           UUID                       NOT NULL,
    title                TEXT                       NOT NULL,
    sku                  extensions.citext,
    status               commerce.variant_status    NOT NULL DEFAULT 'active',
    requires_shipping    BOOLEAN                    NOT NULL DEFAULT true,
    track_inventory      BOOLEAN                    NOT NULL DEFAULT true,
    on_hand_quantity     BIGINT                     NOT NULL DEFAULT 0,
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, product_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variants_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT product_variants_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku::text)) BETWEEN 1 AND 64
    ),
    CONSTRAINT product_variants_sku_characters_check CHECK (
        sku IS NULL OR sku::text !~ '[[:cntrl:]]'
    ),
    CONSTRAINT product_variants_on_hand_nonnegative_check CHECK (
        on_hand_quantity >= 0
    ),
    CONSTRAINT product_variants_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.variant_selected_options (
    store_id             UUID    NOT NULL,
    product_id           UUID    NOT NULL,
    variant_id           UUID    NOT NULL,
    option_id            UUID    NOT NULL,
    option_value_id      UUID    NOT NULL,

    PRIMARY KEY (store_id, variant_id, option_id),
    UNIQUE (store_id, variant_id, option_value_id),
    FOREIGN KEY (store_id, product_id, variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, option_id, option_value_id)
        REFERENCES commerce.product_option_values(store_id,
            product_id,
            option_id,
            id
        ) ON DELETE CASCADE
);

CREATE TABLE commerce.product_publications (
    store_id             UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id, sales_channel_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.collections (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               commerce.collection_status  NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT collections_handle_format_check CHECK (
        handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'
    ),
    CONSTRAINT collections_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT collections_description_length_check CHECK (
        length(description) <= 100000
    ),
    CONSTRAINT collections_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.collection_products (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    position             INTEGER     NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, collection_id, product_id),
    UNIQUE (store_id, collection_id, position),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_products_position_check CHECK (position BETWEEN 0 AND 999)
);

CREATE TABLE commerce.collection_publications (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, collection_id, sales_channel_id),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.media_assets (
    id                   UUID                        NOT NULL PRIMARY KEY,
    store_id             UUID                        NOT NULL,
    product_id           UUID                        NOT NULL,
    product_variant_id   UUID,
    object_key           TEXT                        NOT NULL UNIQUE,
    file_name            TEXT                        NOT NULL,
    media_type           TEXT                        NOT NULL,
    media_kind           commerce.media_kind          NOT NULL,
    byte_size            BIGINT                      NOT NULL,
    sha256_digest        BYTEA                       NOT NULL,
    alt_text             TEXT                        NOT NULL DEFAULT '',
    position             SMALLINT                    NOT NULL,
    status               commerce.media_asset_status  NOT NULL DEFAULT 'pending_upload',
    public_url           TEXT,
    created_by           UUID                        NOT NULL,
    ready_by             UUID,
    archived_by          UUID,
    ready_at             TIMESTAMPTZ,
    archived_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                 NOT NULL,
    updated_at           TIMESTAMPTZ                 NOT NULL,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id),
    FOREIGN KEY (created_by) REFERENCES identity.users(id),
    FOREIGN KEY (ready_by) REFERENCES identity.users(id),
    FOREIGN KEY (archived_by) REFERENCES identity.users(id),
    CONSTRAINT media_assets_object_key_check CHECK (
        length(object_key) BETWEEN 20 AND 255
        AND object_key ~ '^stores/[0-9a-f-]{36}/media/[0-9a-f-]{36}/original$'
    ),
    CONSTRAINT media_assets_file_name_check CHECK (
        length(trim(file_name)) BETWEEN 1 AND 255
        AND file_name !~ '[[:cntrl:]/\\]'
    ),
    CONSTRAINT media_assets_type_kind_check CHECK (
        (media_kind = 'image' AND media_type IN (
            'image/jpeg', 'image/png', 'image/webp', 'image/avif', 'image/gif'
        ) AND byte_size BETWEEN 1 AND 26214400)
        OR (media_kind = 'video' AND media_type IN (
            'video/mp4', 'video/webm'
        ) AND byte_size BETWEEN 1 AND 524288000)
    ),
    CONSTRAINT media_assets_sha256_check CHECK (octet_length(sha256_digest) = 32),
    CONSTRAINT media_assets_alt_text_check CHECK (
        length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'
    ),
    CONSTRAINT media_assets_position_check CHECK (position BETWEEN 0 AND 99),
    CONSTRAINT media_assets_public_url_check CHECK (
        public_url IS NULL OR (length(public_url) BETWEEN 12 AND 2048 AND public_url ~ '^https://')
    ),
    CONSTRAINT media_assets_lifecycle_check CHECK (
        (status = 'pending_upload' AND public_url IS NULL
            AND ready_by IS NULL AND ready_at IS NULL
            AND archived_by IS NULL AND archived_at IS NULL)
        OR (status = 'ready' AND public_url IS NOT NULL
            AND ready_by IS NOT NULL AND ready_at IS NOT NULL
            AND archived_by IS NULL AND archived_at IS NULL)
        OR (status = 'archived' AND archived_by IS NOT NULL AND archived_at IS NOT NULL
            AND ((public_url IS NULL AND ready_by IS NULL AND ready_at IS NULL)
                OR (public_url IS NOT NULL AND ready_by IS NOT NULL AND ready_at IS NOT NULL)))
    )
);

CREATE TABLE commerce.reviews (
    id                   UUID                     NOT NULL PRIMARY KEY,
    store_id             UUID                     NOT NULL,
    product_id           UUID                     NOT NULL,
    parent_review_id     UUID,
    rating               SMALLINT,
    title                TEXT,
    content              TEXT                     NOT NULL,
    author_name          TEXT                     NOT NULL,
    author_email         extensions.citext,
    status               commerce.review_status   NOT NULL DEFAULT 'pending',
    is_staff_reply       BOOLEAN                  NOT NULL DEFAULT false,
    verified_buyer       BOOLEAN                  NOT NULL DEFAULT false,
    approved_by_user_id  UUID,
    approved_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, parent_review_id)
        REFERENCES commerce.reviews(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (approved_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT reviews_rating_shape_check CHECK (
        (is_staff_reply AND rating IS NULL AND parent_review_id IS NOT NULL)
        OR (NOT is_staff_reply AND rating IS NOT NULL AND rating BETWEEN 1 AND 5
            AND parent_review_id IS NULL)
    ),
    CONSTRAINT reviews_content_length_check CHECK (
        length(content) BETWEEN 1 AND 10000
    ),
    CONSTRAINT reviews_title_length_check CHECK (
        title IS NULL OR length(title) <= 255
    ),
    CONSTRAINT reviews_author_name_length_check CHECK (
        length(trim(author_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT reviews_approval_shape_check CHECK (
        (status = 'approved') = (approved_at IS NOT NULL AND approved_by_user_id IS NOT NULL)
    ),
    CONSTRAINT reviews_verified_buyer_requires_approval_check CHECK (
        NOT verified_buyer OR status = 'approved'
    )
);

CREATE INDEX products_store_status_created_idx
    ON commerce.products (store_id, status, created_at DESC, id DESC);

CREATE UNIQUE INDEX product_variants_store_sku_key
    ON commerce.product_variants (store_id, sku)
    WHERE sku IS NOT NULL;

CREATE INDEX product_variants_product_status_idx
    ON commerce.product_variants (store_id, product_id, status);

CREATE INDEX product_publications_channel_product_idx
    ON commerce.product_publications (store_id,
        sales_channel_id,
        product_id
    );

CREATE INDEX collections_store_status_created_idx
    ON commerce.collections (store_id, status, created_at DESC, id DESC
    );

CREATE INDEX collection_products_product_idx
    ON commerce.collection_products (store_id, product_id, collection_id);

CREATE INDEX collection_publications_channel_collection_idx
    ON commerce.collection_publications (store_id, sales_channel_id, collection_id
    );

CREATE UNIQUE INDEX media_assets_product_position_active_idx
    ON commerce.media_assets (store_id, product_id, position)
    WHERE status <> 'archived';

CREATE INDEX media_assets_product_status_position_idx
    ON commerce.media_assets (store_id, product_id, status, position, id
    );

CREATE INDEX reviews_product_status_idx
    ON commerce.reviews (store_id, product_id, status, created_at, id);

CREATE INDEX reviews_parent_idx
    ON commerce.reviews (store_id, parent_review_id)
    WHERE parent_review_id IS NOT NULL;

ALTER TABLE commerce.products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_option_values ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_variants ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.variant_selected_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collections ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_assets ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.reviews ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_option_values
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_variants
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.variant_selected_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collection_products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collection_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.media_assets
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.reviews
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

REVOKE DELETE ON commerce.collections FROM chaos_runtime;

REVOKE DELETE ON commerce.media_assets FROM chaos_runtime;

REVOKE DELETE ON commerce.reviews FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Pricing ===

CREATE TYPE commerce.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TABLE commerce.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    code                 extensions.citext            NOT NULL,
    name                 TEXT                         NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    status               commerce.price_list_status    NOT NULL DEFAULT 'draft',
    starts_at            TIMESTAMPTZ,
    ends_at              TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES commerce.store_currencies(store_id, currency),
    CONSTRAINT price_lists_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$'
    ),
    CONSTRAINT price_lists_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT price_lists_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT price_lists_validity_window_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR ends_at > starts_at
    )
);

CREATE TABLE commerce.prices (
    id                   UUID         NOT NULL PRIMARY KEY,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, price_list_id, product_variant_id),
    FOREIGN KEY (store_id, price_list_id)
        REFERENCES commerce.price_lists(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, id),
    CONSTRAINT prices_amount_nonnegative_check CHECK (
        amount_minor >= 0
    )
);

ALTER TABLE commerce.price_lists
    ADD UNIQUE (store_id, id, currency);

CREATE INDEX price_lists_store_activation_idx
    ON commerce.price_lists (store_id,
        status,
        currency,
        starts_at,
        ends_at
    );

CREATE INDEX prices_variant_lookup_idx
    ON commerce.prices (store_id,
        product_variant_id,
        price_list_id
    );

ALTER TABLE commerce.price_lists ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.prices ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.price_lists
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.prices
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Search read model ===

CREATE TABLE commerce.product_documents (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    document            TSVECTOR    NOT NULL,
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE
);

CREATE INDEX product_documents_search_idx
    ON commerce.product_documents USING GIN (document);

CREATE FUNCTION commerce.refresh_product_document(UUID, UUID)
RETURNS VOID LANGUAGE SQL SECURITY DEFINER SET search_path = pg_catalog AS $$
    INSERT INTO commerce.product_documents (store_id, product_id, document, indexed_at)
    SELECT product.store_id, product.id,
           to_tsvector('simple', concat_ws(
               ' ', product.handle::text, product.title, product.description,
               string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
           )), CURRENT_TIMESTAMP
      FROM commerce.products AS product
      LEFT JOIN commerce.product_variants AS variant
        ON variant.store_id = product.store_id AND variant.product_id = product.id
     WHERE product.store_id = $1 AND product.id = $2
     GROUP BY product.store_id, product.id
    ON CONFLICT (store_id, product_id) DO UPDATE
        SET document = EXCLUDED.document, indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION commerce.capture_product_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO integration.event_outbox (
        id, store_id, aggregate_type, aggregate_id, event_type, payload
    ) VALUES (
        uuidv7(), NEW.store_id, 'product', NEW.id,
        'search.product.changed', jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.capture_variant_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    owning_store_id UUID;
    changed_product_id UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        owning_store_id := OLD.store_id;
        changed_product_id := OLD.product_id;
    ELSE
        owning_store_id := NEW.store_id;
        changed_product_id := NEW.product_id;
    END IF;
    IF EXISTS (
        SELECT 1 FROM commerce.stores
         WHERE id = owning_store_id
    ) THEN
        INSERT INTO integration.event_outbox (
            id, store_id, aggregate_type, aggregate_id, event_type, payload
        ) VALUES (
            uuidv7(), owning_store_id, 'product', changed_product_id,
            'search.product.changed', jsonb_build_object('product_id', changed_product_id)
        );
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.rebuild_store_products(UUID)
RETURNS BIGINT LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE product_id UUID; rebuilt BIGINT := 0;
BEGIN
    DELETE FROM commerce.product_documents WHERE store_id = $1;
    FOR product_id IN SELECT id FROM commerce.products
        WHERE store_id = $1
    LOOP
        PERFORM commerce.refresh_product_document($1, product_id);
        rebuilt := rebuilt + 1;
    END LOOP;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION commerce.process_events(INTEGER, TIMESTAMPTZ)
RETURNS BIGINT LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE event RECORD; processed BIGINT := 0;
BEGIN
    FOR event IN
        SELECT outbox.id,
               outbox.store_id,
               outbox.aggregate_id,
               outbox.attempts
          FROM integration.claim_routed_event_outbox(
                   'chaos_search_events', $1
               ) AS outbox
    LOOP
        BEGIN
            PERFORM commerce.refresh_product_document(
                event.store_id, event.aggregate_id
            );
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, true, '', 8, $2
            );
            processed := processed + 1;
        EXCEPTION WHEN OTHERS THEN
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, false, SQLERRM, 8, $2
            );
        END;
    END LOOP;
    RETURN processed;
END;
$$;

CREATE TRIGGER products_search_change
AFTER INSERT OR UPDATE OF handle, title, description ON commerce.products
FOR EACH ROW EXECUTE FUNCTION commerce.capture_product_change();

CREATE TRIGGER variants_search_change
AFTER INSERT OR UPDATE OF title, sku OR DELETE ON commerce.product_variants
FOR EACH ROW EXECUTE FUNCTION commerce.capture_variant_change();

ALTER TABLE commerce.product_documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.product_documents
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.rebuild_store_products(UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.process_events(INTEGER, TIMESTAMPTZ) TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Sales ===

CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');

CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_payment_status AS ENUM (
    'pending',
    'paid',
    'failed',
    'partially_refunded',
    'refunded'
);

CREATE TYPE commerce.order_shipping_status AS ENUM (
    'pending',
    'shipped',
    'delivered',
    'cancelled'
);

-- Shopper identity, Cart, and Order form the Storefront sales flow.
-- Stripe owns checkout UI, address collection, shipping, tax, and payment
-- collection; Chaos stores the resulting business Order and immutable lines.
CREATE TABLE commerce.shoppers (
    id             UUID        NOT NULL PRIMARY KEY,
    store_id       UUID        NOT NULL,
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE
);

CREATE TABLE commerce.carts (
    id                   UUID                NOT NULL PRIMARY KEY,
    store_id             UUID                NOT NULL,
    sales_channel_id     UUID                NOT NULL,
    shopper_id           UUID                NOT NULL,
    price_list_id        UUID                NOT NULL,
    currency             CHAR(3)             NOT NULL,
    locale               VARCHAR(63)         NOT NULL DEFAULT 'en-US',
    status               commerce.cart_status   NOT NULL DEFAULT 'active',
    version              BIGINT              NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id),
    FOREIGN KEY (store_id, shopper_id)
        REFERENCES commerce.shoppers(store_id, id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT carts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT carts_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT carts_version_nonnegative_check CHECK (version >= 0)
);

CREATE TABLE commerce.cart_lines (
    store_id                UUID        NOT NULL,
    cart_id                 UUID        NOT NULL,
    product_id              UUID        NOT NULL,
    product_variant_id      UUID        NOT NULL,
    product_title           TEXT        NOT NULL,
    variant_title           TEXT        NOT NULL,
    sku                     TEXT,
    requires_shipping       BOOLEAN     NOT NULL,
    track_inventory         BOOLEAN     NOT NULL,
    quantity                INTEGER     NOT NULL,
    unit_price_amount_minor BIGINT      NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, cart_id, product_variant_id),
    FOREIGN KEY (store_id, cart_id)
        REFERENCES commerce.carts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id),
    CONSTRAINT cart_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT cart_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT cart_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT cart_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT cart_lines_unit_price_nonnegative_check CHECK (unit_price_amount_minor >= 0)
);

CREATE TABLE commerce.orders (
    id                       UUID                               NOT NULL PRIMARY KEY,
    store_id                 UUID                               NOT NULL,
    order_number             TEXT                               NOT NULL,
    sales_channel_id         UUID                               NOT NULL,
    cart_id                  UUID                               NOT NULL,
    shopper_id               UUID                               NOT NULL,
    price_list_id            UUID                               NOT NULL,
    currency                 CHAR(3)                            NOT NULL,
    locale                   VARCHAR(63)                        NOT NULL DEFAULT 'en-US',
    status                   commerce.order_status              NOT NULL DEFAULT 'pending',
    payment_status           commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    shipping_status          commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    stripe_checkout_session_id TEXT,
    stripe_payment_intent_id TEXT,
    stripe_charge_id         TEXT,
    stripe_refund_id         TEXT,
    payment_failure_code     TEXT,
    refunded_amount_minor    BIGINT                             NOT NULL DEFAULT 0,
    shipping_provider        TEXT,
    shipping_provider_reference TEXT,
    shipping_tracking_number TEXT,
    shipping_tracking_url    TEXT,
    subtotal_amount_minor    BIGINT                             NOT NULL,
    discount_amount_minor    BIGINT                             NOT NULL,
    tax_amount_minor         BIGINT                             NOT NULL,
    shipping_amount_minor    BIGINT                             NOT NULL,
    total_amount_minor       BIGINT                             NOT NULL,
    contact_email            extensions.citext,
    contact_phone            TEXT,
    billing_full_name        TEXT,
    billing_company          TEXT,
    billing_address_line1    TEXT,
    billing_address_line2    TEXT,
    billing_locality         TEXT,
    billing_administrative_area TEXT,
    billing_postal_code      TEXT,
    billing_country_code     CHAR(2),
    shipping_full_name       TEXT,
    shipping_company         TEXT,
    shipping_address_line1   TEXT,
    shipping_address_line2   TEXT,
    shipping_locality        TEXT,
    shipping_administrative_area TEXT,
    shipping_postal_code     TEXT,
    shipping_country_code    CHAR(2),
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, order_number),
    UNIQUE (store_id, id, shopper_id),
    FOREIGN KEY (store_id, cart_id)
        REFERENCES commerce.carts(store_id, id),
    FOREIGN KEY (store_id, shopper_id)
        REFERENCES commerce.shoppers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT orders_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_order_number_check CHECK (
        order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'
    ),
    CONSTRAINT orders_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT orders_amounts_check CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor >= 0
        AND refunded_amount_minor >= 0
        AND refunded_amount_minor <= total_amount_minor
    ),
    CONSTRAINT orders_contact_email_length_check CHECK (
        contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT orders_contact_phone_format_check CHECK (
        contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'
    ),
    CONSTRAINT orders_billing_country_code_check CHECK (
        billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT orders_shipping_country_code_check CHECK (
        shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT orders_stripe_checkout_session_check CHECK (
        stripe_checkout_session_id IS NULL
        OR length(trim(stripe_checkout_session_id)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_stripe_payment_intent_check CHECK (
        stripe_payment_intent_id IS NULL OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_stripe_charge_check CHECK (
        stripe_charge_id IS NULL OR stripe_charge_id ~ '^ch_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_stripe_refund_check CHECK (
        stripe_refund_id IS NULL OR stripe_refund_id ~ '^re_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_payment_failure_code_check CHECK (
        payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000
    ),
    CONSTRAINT orders_shipping_provider_check CHECK (
        shipping_provider IS NULL OR length(trim(shipping_provider)) BETWEEN 1 AND 64
    ),
    CONSTRAINT orders_shipping_reference_check CHECK (
        shipping_provider_reference IS NULL
        OR length(trim(shipping_provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_shipping_tracking_number_check CHECK (
        shipping_tracking_number IS NULL
        OR length(trim(shipping_tracking_number)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_shipping_tracking_url_check CHECK (
        shipping_tracking_url IS NULL
        OR (length(shipping_tracking_url) BETWEEN 9 AND 2048 AND shipping_tracking_url ~ '^https://')
    )
);

CREATE TABLE commerce.order_tracking_tokens (
    store_id       UUID        NOT NULL,
    order_id       UUID        NOT NULL,
    token_digest   BYTEA       NOT NULL,
    expires_at     TIMESTAMPTZ NOT NULL,
    last_used_at   TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, order_id),
    UNIQUE (store_id, token_digest),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_tokens_digest_check CHECK (octet_length(token_digest) = 32),
    CONSTRAINT order_tracking_tokens_expiry_check CHECK (expires_at > created_at)
);

CREATE TABLE commerce.order_lines (
    store_id                 UUID        NOT NULL,
    order_id                 UUID        NOT NULL,
    position                 SMALLINT    NOT NULL,
    product_id               UUID        NOT NULL,
    product_variant_id       UUID        NOT NULL,
    product_title            TEXT        NOT NULL,
    variant_title            TEXT        NOT NULL,
    sku                      TEXT,
    requires_shipping        BOOLEAN     NOT NULL,
    track_inventory          BOOLEAN     NOT NULL,
    quantity                 INTEGER     NOT NULL,
    unit_price_amount_minor  BIGINT      NOT NULL,
    subtotal_amount_minor    BIGINT      NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, order_id, position),
    UNIQUE (store_id, order_id, product_variant_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    CONSTRAINT order_lines_position_check CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT order_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT order_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT order_lines_amounts_check CHECK (
        unit_price_amount_minor >= 0
        AND subtotal_amount_minor = unit_price_amount_minor * quantity
        AND subtotal_amount_minor >= 0
    )
);

CREATE TABLE commerce.order_transitions (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    order_id             UUID                         NOT NULL,
    from_status          commerce.order_status,
    to_status            commerce.order_status           NOT NULL,
    kind                 commerce.order_transition_kind NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                  NOT NULL,

    UNIQUE (store_id, order_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT order_transitions_shape_check CHECK (
        (kind = 'created' AND from_status IS NULL AND to_status = 'pending')
        OR (kind = 'confirmed' AND from_status = 'pending' AND to_status = 'confirmed')
        OR (kind = 'cancelled' AND from_status = 'pending' AND to_status = 'cancelled')
    )
);

ALTER TABLE commerce.orders
    ADD UNIQUE (store_id, id, currency);

-- === Payments ===

CREATE TABLE commerce.payment_provider_accounts (
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    provider                   TEXT        NOT NULL,
    display_name               TEXT        NOT NULL DEFAULT 'Payment provider',
    credential_secret_reference TEXT,
    previous_credential_secret_reference TEXT,
    credential_rotation_expires_at TIMESTAMPTZ,
    webhook_secret_reference    TEXT,
    previous_webhook_secret_reference TEXT,
    webhook_rotation_expires_at TIMESTAMPTZ,
    readiness_status            TEXT        NOT NULL DEFAULT 'unchecked',
    readiness_snapshot          JSONB,
    readiness_checked_at        TIMESTAMPTZ,
    readiness_valid_until       TIMESTAMPTZ,
    readiness_reconcile_at      TIMESTAMPTZ,
    readiness_locked_by         UUID,
    readiness_locked_at         TIMESTAMPTZ,
    readiness_reconcile_attempts INTEGER     NOT NULL DEFAULT 0,
    readiness_last_error        TEXT,
    enabled                    BOOLEAN     NOT NULL DEFAULT false,
    created_by_user_id          UUID,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    CONSTRAINT payment_provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT payment_provider_accounts_provider_length_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT payment_provider_accounts_stripe_only_check CHECK (
        provider = 'stripe_checkout'
    ),
    CONSTRAINT payment_provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT payment_provider_accounts_credential_reference_check CHECK (
        credential_secret_reference IS NULL
        OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_webhook_reference_check CHECK (
        webhook_secret_reference IS NULL
        OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(webhook_secret_reference) <= 32768
            AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_previous_webhook_reference_check CHECK (
        previous_webhook_secret_reference IS NULL
        OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_webhook_secret_reference) <= 32768
            AND previous_webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_webhook_rotation_shape_check CHECK (
        (previous_webhook_secret_reference IS NULL AND webhook_rotation_expires_at IS NULL)
        OR (previous_webhook_secret_reference IS NOT NULL AND webhook_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_readiness_status_check CHECK (
        readiness_status IN ('unchecked', 'ready', 'action_required')
    ),
    CONSTRAINT payment_provider_accounts_readiness_shape_check CHECK (
        (
            readiness_status = 'unchecked'
            AND readiness_snapshot IS NULL
            AND readiness_checked_at IS NULL
            AND readiness_valid_until IS NULL
            AND readiness_reconcile_at IS NULL
        )
        OR (
            readiness_status <> 'unchecked'
            AND jsonb_typeof(readiness_snapshot) = 'object'
            AND pg_column_size(readiness_snapshot) <= 8192
            AND readiness_checked_at IS NOT NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_enabled_readiness_check CHECK (
        NOT enabled
        OR (
            readiness_status = 'ready'
            AND readiness_valid_until IS NOT NULL
            AND readiness_reconcile_at IS NOT NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_readiness_validity_check CHECK (
        (
            readiness_status = 'ready'
            AND readiness_valid_until > readiness_checked_at
            AND readiness_reconcile_at IS NOT NULL
        )
        OR (
            readiness_status <> 'ready'
            AND readiness_valid_until IS NULL
            AND readiness_reconcile_at IS NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_readiness_lock_shape_check CHECK (
        (readiness_locked_by IS NULL AND readiness_locked_at IS NULL)
        OR (readiness_locked_by IS NOT NULL AND readiness_locked_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_readiness_attempts_check CHECK (
        readiness_reconcile_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT payment_provider_accounts_readiness_error_length_check CHECK (
        readiness_last_error IS NULL OR length(readiness_last_error) BETWEEN 1 AND 2000
    )
);

-- === Indexes ===

CREATE INDEX shoppers_store_seen_idx
    ON commerce.shoppers (store_id, last_seen_at DESC, id DESC);

CREATE INDEX carts_channel_updated_idx
    ON commerce.carts (store_id,
        sales_channel_id,
        status,
        updated_at DESC,
        id DESC
    );

CREATE INDEX cart_lines_variant_lookup_idx
    ON commerce.cart_lines (store_id, product_variant_id, cart_id);

CREATE INDEX orders_channel_created_idx
    ON commerce.orders (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX order_tracking_tokens_expiry_idx
    ON commerce.order_tracking_tokens (expires_at, store_id, order_id);

CREATE INDEX order_transitions_order_time_idx
    ON commerce.order_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

CREATE INDEX payment_provider_accounts_store_created_idx
    ON commerce.payment_provider_accounts (store_id, created_at DESC, id DESC);

CREATE INDEX payment_provider_accounts_readiness_due_idx
    ON commerce.payment_provider_accounts (readiness_reconcile_at, id)
    WHERE enabled;

-- === Payment provider workflows ===

CREATE FUNCTION commerce.resolve_provider_account(
    requested_provider             TEXT,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id            UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.store_id
      FROM commerce.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider             TEXT,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    secret_reference    TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, candidate.secret_reference
      FROM commerce.payment_provider_accounts AS account
      CROSS JOIN LATERAL (
          VALUES
              (account.webhook_secret_reference, 0),
              (
                  CASE WHEN account.webhook_rotation_expires_at > CURRENT_TIMESTAMP
                       THEN account.previous_webhook_secret_reference END,
                  1
              )
      ) AS candidate(secret_reference, priority)
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled
       AND candidate.secret_reference IS NOT NULL
     ORDER BY candidate.priority;
$$;

CREATE FUNCTION commerce.claim_provider_readiness_checks(
    worker_id   UUID,
    batch_size  INTEGER,
    claimed_at  TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id       UUID,
    store_id                  UUID,
    provider                  TEXT,
    credential_secret_reference TEXT,
    attempts                  INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE commerce.payment_provider_accounts AS account
           SET enabled = false,
               readiness_status = 'action_required',
               readiness_snapshot = jsonb_set(
                   jsonb_set(account.readiness_snapshot, '{ready}', 'false'::jsonb, true),
                   '{blocker_codes}', '["readiness_expired"]'::jsonb, true
               ),
               readiness_valid_until = NULL,
               readiness_reconcile_at = NULL,
               readiness_locked_by = NULL,
               readiness_locked_at = NULL,
               readiness_last_error = NULL,
               updated_at = claimed_at
         WHERE account.enabled
           AND account.readiness_valid_until <= claimed_at
        RETURNING account.id
    ), claimable AS (
        SELECT account.id
          FROM commerce.payment_provider_accounts AS account
         WHERE account.enabled
           AND account.credential_secret_reference IS NOT NULL
           AND account.readiness_valid_until > claimed_at
           AND account.readiness_reconcile_at <= claimed_at
           AND (
               account.readiness_locked_at IS NULL
               OR account.readiness_locked_at <= stale_before
           )
         ORDER BY account.readiness_reconcile_at, account.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE commerce.payment_provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

CREATE FUNCTION commerce.finish_provider_readiness_check(
    requested_provider_account_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    ready BOOLEAN,
    requested_readiness_snapshot JSONB,
    observed_at TIMESTAMPTZ,
    failure_message TEXT
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE commerce.payment_provider_accounts AS account
       SET enabled = CASE
               WHEN succeeded THEN account.enabled AND ready
               ELSE account.enabled
           END,
           readiness_status = CASE
               WHEN succeeded AND ready THEN 'ready'
               WHEN succeeded THEN 'action_required'
               ELSE account.readiness_status
           END,
           readiness_snapshot = CASE
               WHEN succeeded THEN requested_readiness_snapshot
               ELSE account.readiness_snapshot
           END,
           readiness_checked_at = CASE
               WHEN succeeded THEN observed_at
               ELSE account.readiness_checked_at
           END,
           readiness_valid_until = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '24 hours'
               WHEN succeeded THEN NULL
               ELSE account.readiness_valid_until
           END,
           readiness_reconcile_at = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '6 hours'
               WHEN succeeded THEN NULL
               ELSE observed_at + make_interval(
                   secs => least(power(2, greatest(account.readiness_reconcile_attempts - 1, 0))::integer, 3600)
               )
           END,
           readiness_locked_by = NULL,
           readiness_locked_at = NULL,
           readiness_reconcile_attempts = CASE
               WHEN succeeded THEN 0
               ELSE account.readiness_reconcile_attempts
           END,
           readiness_last_error = CASE
               WHEN succeeded THEN NULL
               ELSE COALESCE(NULLIF(left(failure_message, 2000), ''), 'readiness check failed')
           END,
           updated_at = CASE WHEN succeeded THEN observed_at ELSE account.updated_at END
     WHERE account.id = requested_provider_account_id
       AND account.readiness_locked_by = worker_id
    RETURNING true;
$$;

-- === Row-level security ===

ALTER TABLE commerce.shoppers ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.shoppers FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.shoppers
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_tracking_tokens ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_transitions ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.carts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.cart_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.orders
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_tracking_tokens
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_transitions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) TO chaos_runtime;

GRANT EXECUTE
    ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
)
    TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE DELETE ON commerce.orders FROM chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
