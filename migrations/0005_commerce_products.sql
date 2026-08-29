CREATE TYPE commerce.product_status AS ENUM ('draft', 'active', 'archived');
CREATE TYPE commerce.variant_status AS ENUM ('active', 'archived');
CREATE TYPE commerce.collection_status AS ENUM ('draft', 'active', 'archived');
CREATE TYPE commerce.media_kind AS ENUM ('image', 'video');
CREATE TYPE commerce.media_asset_status AS ENUM ('pending_upload', 'ready', 'archived');
CREATE TYPE commerce.review_status AS ENUM ('pending', 'approved', 'rejected');
CREATE TYPE commerce.review_origin AS ENUM ('storefront', 'manual');
CREATE TYPE commerce.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TABLE commerce.products (
    id          UUID                       NOT NULL PRIMARY KEY,
    store_id    UUID                       NOT NULL,
    handle      extensions.citext          NOT NULL,
    title       TEXT                       NOT NULL,
    description TEXT                       NOT NULL DEFAULT '',
    status      commerce.product_status    NOT NULL DEFAULT 'draft',
    meta        JSONB,
    revision    BIGINT                     NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT products_store_id_handle_key        UNIQUE (store_id, handle),
    CONSTRAINT products_store_id_id_key            UNIQUE (store_id, id),
    CONSTRAINT products_store_id_fkey              FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT products_handle_format_check        CHECK (handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'),
    CONSTRAINT products_title_length_check         CHECK (length(trim(title)) BETWEEN 1 AND 255),
    CONSTRAINT products_description_length_check   CHECK (length(description) <= 100000),
    CONSTRAINT products_meta_size_check            CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT products_meta_is_object_check       CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT products_revision_nonnegative_check CHECK (revision >= 0)
);

CREATE TABLE commerce.product_options (
    id          UUID              NOT NULL PRIMARY KEY,
    store_id    UUID              NOT NULL,
    product_id  UUID              NOT NULL,
    name        extensions.citext NOT NULL,
    position    SMALLINT          NOT NULL,
    archived_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_options_store_id_product_id_id_key          UNIQUE (store_id, product_id, id),
    CONSTRAINT product_options_store_id_product_id_fkey            FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_options_name_length_check                   CHECK (length(trim(name::text)) BETWEEN 1 AND 80),
    CONSTRAINT product_options_position_check                      CHECK (position BETWEEN 0 AND 9)
);

CREATE TABLE commerce.product_option_values (
    id          UUID              NOT NULL PRIMARY KEY,
    store_id    UUID              NOT NULL,
    product_id  UUID              NOT NULL,
    option_id   UUID              NOT NULL,
    value       extensions.citext NOT NULL,
    position    SMALLINT          NOT NULL,
    archived_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_option_values_store_id_product_id_option_id_id_key          UNIQUE (store_id, product_id, option_id, id),
    CONSTRAINT product_option_values_store_id_product_id_option_id_fkey            FOREIGN KEY (store_id, product_id, option_id) REFERENCES commerce.product_options (store_id, product_id, id) ON DELETE CASCADE,
    CONSTRAINT product_option_values_value_length_check                            CHECK (length(trim(value::text)) BETWEEN 1 AND 120),
    CONSTRAINT product_option_values_position_check                                CHECK (position BETWEEN 0 AND 999)
);

CREATE TABLE commerce.product_variants (
    id                UUID                       NOT NULL PRIMARY KEY,
    store_id          UUID                       NOT NULL,
    product_id        UUID                       NOT NULL,
    title             TEXT                       NOT NULL,
    sku               extensions.citext,
    status            commerce.variant_status    NOT NULL DEFAULT 'active',
    track_inventory   BOOLEAN                    NOT NULL DEFAULT true,
    on_hand_quantity  BIGINT                     NOT NULL DEFAULT 0,
    reserved_quantity BIGINT                     NOT NULL DEFAULT 0,
    meta              JSONB,
    created_at        TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at        TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_variants_store_id_id_key                  UNIQUE (store_id, id),
    CONSTRAINT product_variants_store_id_product_id_id_key       UNIQUE (store_id, product_id, id),
    CONSTRAINT product_variants_store_id_product_id_fkey         FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variants_title_length_check               CHECK (length(trim(title)) BETWEEN 1 AND 255),
    CONSTRAINT product_variants_sku_length_check                 CHECK (sku IS NULL OR length(trim(sku::text)) BETWEEN 1 AND 64),
    CONSTRAINT product_variants_sku_characters_check             CHECK (sku IS NULL OR sku::text !~ '[[:cntrl:]]'),
    CONSTRAINT product_variants_on_hand_nonnegative_check        CHECK (on_hand_quantity >= 0),
    CONSTRAINT product_variants_reserved_nonnegative_check       CHECK (reserved_quantity >= 0),
    CONSTRAINT product_variants_reserved_not_above_on_hand_check CHECK (reserved_quantity <= on_hand_quantity),
    CONSTRAINT product_variants_meta_size_check                  CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT product_variants_meta_is_object_check             CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object')
);

CREATE TABLE commerce.variant_selected_options (
    store_id         UUID NOT NULL,
    product_id       UUID NOT NULL,
    variant_id       UUID NOT NULL,
    option_id        UUID NOT NULL,
    option_value_id  UUID NOT NULL,

    CONSTRAINT variant_selected_options_pkey                                       PRIMARY KEY (store_id, variant_id, option_id),
    CONSTRAINT variant_selected_options_store_id_variant_id_option_value_id_key    UNIQUE (store_id, variant_id, option_value_id),
    CONSTRAINT variant_selected_options_store_id_product_id_variant_fkey           FOREIGN KEY (store_id, product_id, variant_id) REFERENCES commerce.product_variants (store_id, product_id, id) ON DELETE CASCADE,
    CONSTRAINT variant_selected_options_store_id_product_id_option_value_fkey      FOREIGN KEY (store_id, product_id, option_id, option_value_id) REFERENCES commerce.product_option_values (store_id, product_id, option_id, id) ON DELETE CASCADE
);

CREATE TABLE commerce.product_publications (
    store_id          UUID        NOT NULL,
    product_id        UUID        NOT NULL,
    sales_channel_id  UUID        NOT NULL,
    published_at      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_publications_pkey                        PRIMARY KEY (store_id, product_id, sales_channel_id),
    CONSTRAINT product_publications_store_id_product_fkey       FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_publications_store_id_sales_channel_fkey FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.store_sales_channels (store_id, id) ON DELETE CASCADE
);

CREATE TABLE commerce.collections (
    id          UUID                       NOT NULL PRIMARY KEY,
    store_id    UUID                       NOT NULL,
    handle      extensions.citext          NOT NULL,
    title       TEXT                       NOT NULL,
    description TEXT                       NOT NULL DEFAULT '',
    status      commerce.collection_status NOT NULL DEFAULT 'draft',
    meta        JSONB,
    created_at  TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT collections_store_id_handle_key        UNIQUE (store_id, handle),
    CONSTRAINT collections_store_id_id_key            UNIQUE (store_id, id),
    CONSTRAINT collections_store_id_fkey              FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT collections_handle_format_check        CHECK (handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'),
    CONSTRAINT collections_title_length_check         CHECK (length(trim(title)) BETWEEN 1 AND 255),
    CONSTRAINT collections_description_length_check   CHECK (length(description) <= 100000),
    CONSTRAINT collections_meta_size_check            CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT collections_meta_is_object_check       CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object')
);

CREATE TABLE commerce.collection_products (
    store_id       UUID        NOT NULL,
    collection_id  UUID        NOT NULL,
    product_id     UUID        NOT NULL,
    position       INTEGER     NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT collection_products_pkey                                   PRIMARY KEY (store_id, collection_id, product_id),
    CONSTRAINT collection_products_store_id_collection_id_position_key    UNIQUE (store_id, collection_id, position),
    CONSTRAINT collection_products_store_id_collection_fkey               FOREIGN KEY (store_id, collection_id) REFERENCES commerce.collections (store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_products_store_id_product_fkey                  FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_products_position_check                         CHECK (position BETWEEN 0 AND 999)
);

CREATE TABLE commerce.collection_publications (
    store_id          UUID        NOT NULL,
    collection_id     UUID        NOT NULL,
    sales_channel_id  UUID        NOT NULL,
    published_at      TIMESTAMPTZ NOT NULL,

    CONSTRAINT collection_publications_pkey                        PRIMARY KEY (store_id, collection_id, sales_channel_id),
    CONSTRAINT collection_publications_store_id_collection_fkey    FOREIGN KEY (store_id, collection_id) REFERENCES commerce.collections (store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_publications_store_id_sales_channel_fkey FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.store_sales_channels (store_id, id) ON DELETE CASCADE
);

CREATE TABLE commerce.media_assets (
    id                 UUID                        NOT NULL PRIMARY KEY,
    store_id           UUID                        NOT NULL,
    object_key         TEXT                        NOT NULL UNIQUE,
    file_name          TEXT                        NOT NULL,
    media_type         TEXT                        NOT NULL,
    media_kind         commerce.media_kind         NOT NULL,
    byte_size          BIGINT                      NOT NULL,
    sha256_digest      BYTEA                       NOT NULL,
    status             commerce.media_asset_status NOT NULL DEFAULT 'pending_upload',
    public_url         TEXT,
    ready_at           TIMESTAMPTZ,
    archived_at        TIMESTAMPTZ,
    created_at         TIMESTAMPTZ                 NOT NULL,
    updated_at         TIMESTAMPTZ                 NOT NULL,

    CONSTRAINT media_assets_store_id_id_key        UNIQUE (store_id, id),
    CONSTRAINT media_assets_store_id_fkey          FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT media_assets_object_key_check       CHECK (length(object_key) BETWEEN 20 AND 255 AND object_key ~ '^stores/[0-9a-f-]{36}/media/[0-9a-f-]{36}/original$'),
    CONSTRAINT media_assets_file_name_check        CHECK (length(trim(file_name)) BETWEEN 1 AND 255 AND file_name !~ '[[:cntrl:]/\\]'),
    CONSTRAINT media_assets_type_kind_check        CHECK ((media_kind = 'image' AND media_type IN ('image/jpeg', 'image/png', 'image/webp', 'image/avif', 'image/gif') AND byte_size BETWEEN 1 AND 26214400) OR (media_kind = 'video' AND media_type IN ('video/mp4', 'video/webm') AND byte_size BETWEEN 1 AND 524288000)),
    CONSTRAINT media_assets_sha256_check           CHECK (octet_length(sha256_digest) = 32),
    CONSTRAINT media_assets_public_url_check       CHECK (public_url IS NULL OR (length(public_url) BETWEEN 12 AND 2048 AND public_url ~ '^https://')),
    CONSTRAINT media_assets_lifecycle_check        CHECK ((status = 'pending_upload' AND public_url IS NULL AND ready_at IS NULL AND archived_at IS NULL) OR (status = 'ready' AND public_url IS NOT NULL AND ready_at IS NOT NULL AND archived_at IS NULL) OR (status = 'archived' AND archived_at IS NOT NULL AND ((public_url IS NULL AND ready_at IS NULL) OR (public_url IS NOT NULL AND ready_at IS NOT NULL))))
);

CREATE TABLE commerce.product_media_assets (
    store_id       UUID         NOT NULL,
    product_id     UUID         NOT NULL,
    media_asset_id UUID         NOT NULL,
    alt_text       TEXT         NOT NULL DEFAULT '',
    position       SMALLINT     NOT NULL,
    archived_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_media_assets_pkey                      PRIMARY KEY (store_id, product_id, media_asset_id),
    CONSTRAINT product_media_assets_store_id_product_fkey     FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_media_assets_store_id_media_asset_fkey FOREIGN KEY (store_id, media_asset_id) REFERENCES commerce.media_assets (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_media_assets_alt_text_check            CHECK (length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'),
    CONSTRAINT product_media_assets_position_check            CHECK (position BETWEEN 0 AND 99)
);

CREATE TABLE commerce.product_option_value_media_assets (
    store_id        UUID         NOT NULL,
    product_id      UUID         NOT NULL,
    option_id       UUID         NOT NULL,
    option_value_id UUID         NOT NULL,
    media_asset_id  UUID         NOT NULL,
    alt_text        TEXT         NOT NULL DEFAULT '',
    position        SMALLINT     NOT NULL,
    archived_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_option_value_media_assets_pkey               PRIMARY KEY (store_id, product_id, option_id, option_value_id, media_asset_id),
    CONSTRAINT product_option_value_media_assets_product_fkey       FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_option_value_media_assets_option_value_fkey  FOREIGN KEY (store_id, product_id, option_id, option_value_id) REFERENCES commerce.product_option_values (store_id, product_id, option_id, id) ON DELETE CASCADE,
    CONSTRAINT product_option_value_media_assets_media_asset_fkey   FOREIGN KEY (store_id, media_asset_id) REFERENCES commerce.media_assets (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_option_value_media_assets_alt_text_check     CHECK (length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'),
    CONSTRAINT product_option_value_media_assets_position_check     CHECK (position BETWEEN 0 AND 99)
);

CREATE TABLE commerce.product_variant_media_assets (
    store_id           UUID         NOT NULL,
    product_id         UUID         NOT NULL,
    product_variant_id UUID         NOT NULL,
    media_asset_id     UUID         NOT NULL,
    alt_text           TEXT         NOT NULL DEFAULT '',
    position           SMALLINT     NOT NULL,
    archived_at        TIMESTAMPTZ,
    created_at         TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_variant_media_assets_pkey             PRIMARY KEY (store_id, product_id, product_variant_id, media_asset_id),
    CONSTRAINT product_variant_media_assets_product_fkey     FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variant_media_assets_variant_fkey     FOREIGN KEY (store_id, product_id, product_variant_id) REFERENCES commerce.product_variants (store_id, product_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variant_media_assets_media_asset_fkey FOREIGN KEY (store_id, media_asset_id) REFERENCES commerce.media_assets (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variant_media_assets_alt_text_check   CHECK (length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'),
    CONSTRAINT product_variant_media_assets_position_check   CHECK (position BETWEEN 0 AND 99)
);

CREATE TABLE commerce.product_meta_media_assets (
    store_id       UUID        NOT NULL,
    product_id     UUID        NOT NULL,
    media_asset_id UUID        NOT NULL,
    meta_path      TEXT        NOT NULL,
    alt_text       TEXT        NOT NULL DEFAULT '',
    archived_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_meta_media_assets_pkey                      PRIMARY KEY (store_id, product_id, meta_path, media_asset_id),
    CONSTRAINT product_meta_media_assets_store_id_product_fkey     FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_meta_media_assets_store_id_media_asset_fkey FOREIGN KEY (store_id, media_asset_id) REFERENCES commerce.media_assets (store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_meta_media_assets_path_check                CHECK (length(meta_path) BETWEEN 2 AND 512 AND left(meta_path, 1) = '/' AND meta_path !~ '[[:cntrl:]]'),
    CONSTRAINT product_meta_media_assets_alt_text_check            CHECK (length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]')
);

CREATE TABLE commerce.reviews (
    id                            UUID                    NOT NULL PRIMARY KEY,
    store_id                      UUID                    NOT NULL,
    product_id                    UUID                    NOT NULL,
    parent_review_id              UUID,
    rating                        SMALLINT,
    title                         TEXT,
    content                       TEXT                    NOT NULL,
    author_name                   TEXT                    NOT NULL,
    author_email                  extensions.citext,
    status                        commerce.review_status  NOT NULL DEFAULT 'pending',
    is_staff_reply                BOOLEAN                 NOT NULL DEFAULT false,
    verified_buyer                BOOLEAN                 NOT NULL DEFAULT false,
    origin                        commerce.review_origin  NOT NULL DEFAULT 'storefront',
    source_channel                TEXT,
    source_reference              TEXT,
    publication_consent_confirmed BOOLEAN                 NOT NULL DEFAULT true,
    created_by_user_id            UUID,
    approved_at                   TIMESTAMPTZ,
    created_at                    TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT reviews_store_id_id_key                        UNIQUE (store_id, id),
    CONSTRAINT reviews_store_id_product_id_id_key             UNIQUE (store_id, product_id, id),
    CONSTRAINT reviews_store_id_product_fkey                  FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE,
    CONSTRAINT reviews_store_id_product_id_parent_review_fkey FOREIGN KEY (store_id, product_id, parent_review_id) REFERENCES commerce.reviews (store_id, product_id, id) ON DELETE CASCADE,
    CONSTRAINT reviews_rating_shape_check                     CHECK ((is_staff_reply AND rating IS NULL AND parent_review_id IS NOT NULL) OR (NOT is_staff_reply AND rating IS NOT NULL AND rating BETWEEN 1 AND 5 AND parent_review_id IS NULL)),
    CONSTRAINT reviews_content_length_check                   CHECK (length(content) BETWEEN 1 AND 10000),
    CONSTRAINT reviews_title_length_check                     CHECK (title IS NULL OR length(title) <= 255),
    CONSTRAINT reviews_author_name_length_check               CHECK (length(trim(author_name)) BETWEEN 1 AND 120),
    CONSTRAINT reviews_approval_shape_check                   CHECK ((status = 'approved') = (approved_at IS NOT NULL)),
    CONSTRAINT reviews_verified_buyer_requires_approval_check CHECK (NOT verified_buyer OR status = 'approved'),
    CONSTRAINT reviews_source_channel_check                   CHECK (source_channel IS NULL OR (length(trim(source_channel)) BETWEEN 1 AND 80 AND source_channel !~ '[[:cntrl:]]')),
    CONSTRAINT reviews_source_reference_check                 CHECK (source_reference IS NULL OR (length(trim(source_reference)) BETWEEN 1 AND 255 AND source_reference !~ '[[:cntrl:]]')),
    CONSTRAINT reviews_created_by_user_fkey                   FOREIGN KEY (created_by_user_id) REFERENCES identity.users (id),
    CONSTRAINT reviews_origin_provenance_check                CHECK (
        (
            is_staff_reply
            AND origin = 'storefront'
            AND source_channel IS NULL
            AND source_reference IS NULL
            AND created_by_user_id IS NULL
            AND publication_consent_confirmed
        )
        OR (
            NOT is_staff_reply
            AND origin = 'manual'
            AND source_channel IS NOT NULL
            AND created_by_user_id IS NOT NULL
            AND publication_consent_confirmed
        )
        OR (
            NOT is_staff_reply
            AND origin = 'storefront'
            AND source_channel IS NULL
            AND source_reference IS NULL
            AND created_by_user_id IS NULL
            AND publication_consent_confirmed
        )
    )
);

CREATE TABLE commerce.review_media_assets (
    store_id       UUID        NOT NULL,
    review_id      UUID        NOT NULL,
    media_asset_id UUID        NOT NULL,
    alt_text       TEXT        NOT NULL DEFAULT '',
    position       SMALLINT    NOT NULL,
    archived_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT review_media_assets_pkey                      PRIMARY KEY (store_id, review_id, media_asset_id),
    CONSTRAINT review_media_assets_store_id_review_fkey      FOREIGN KEY (store_id, review_id) REFERENCES commerce.reviews (store_id, id) ON DELETE CASCADE,
    CONSTRAINT review_media_assets_store_id_media_asset_fkey FOREIGN KEY (store_id, media_asset_id) REFERENCES commerce.media_assets (store_id, id) ON DELETE CASCADE,
    CONSTRAINT review_media_assets_alt_text_check            CHECK (length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'),
    CONSTRAINT review_media_assets_position_check            CHECK (position BETWEEN 0 AND 99)
);

CREATE TABLE commerce.product_documents (
    store_id    UUID        NOT NULL,
    product_id  UUID        NOT NULL,
    document    TSVECTOR    NOT NULL,
    indexed_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT product_documents_pkey                  PRIMARY KEY (store_id, product_id),
    CONSTRAINT product_documents_store_id_product_fkey FOREIGN KEY (store_id, product_id) REFERENCES commerce.products (store_id, id) ON DELETE CASCADE
);

CREATE TABLE commerce.price_lists (
    id          UUID                         NOT NULL PRIMARY KEY,
    store_id    UUID                         NOT NULL,
    code        extensions.citext            NOT NULL,
    name        TEXT                         NOT NULL,
    currency    CHAR(3)                      NOT NULL,
    status      commerce.price_list_status   NOT NULL DEFAULT 'draft',
    starts_at   TIMESTAMPTZ,
    ends_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT price_lists_store_id_id_key              UNIQUE (store_id, id),
    CONSTRAINT price_lists_store_id_code_key            UNIQUE (store_id, code),
    CONSTRAINT price_lists_store_id_id_currency_key     UNIQUE (store_id, id, currency),
    CONSTRAINT price_lists_store_id_fkey                FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT price_lists_code_format_check            CHECK (code::text ~ '^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$'),
    CONSTRAINT price_lists_name_length_check            CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT price_lists_currency_format_check        CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT price_lists_validity_window_check        CHECK (starts_at IS NULL OR ends_at IS NULL OR ends_at > starts_at)
);

CREATE TABLE commerce.price_list_items (
    id                   UUID         NOT NULL PRIMARY KEY,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT prices_store_id_price_list_id_product_variant_id_key    UNIQUE (store_id, price_list_id, product_variant_id),
    CONSTRAINT prices_store_id_price_list_fkey                         FOREIGN KEY (store_id, price_list_id) REFERENCES commerce.price_lists (store_id, id) ON DELETE CASCADE,
    CONSTRAINT prices_store_id_product_variant_fkey                    FOREIGN KEY (store_id, product_variant_id) REFERENCES commerce.product_variants (store_id, id),
    CONSTRAINT prices_amount_nonnegative_check                         CHECK (amount_minor >= 0)
);

CREATE INDEX products_store_status_created_idx ON commerce.products (store_id, status, created_at DESC, id DESC);
CREATE UNIQUE INDEX product_variants_store_sku_key ON commerce.product_variants (store_id, sku) WHERE sku IS NOT NULL;
CREATE INDEX product_variants_product_status_idx ON commerce.product_variants (store_id, product_id, status);
CREATE UNIQUE INDEX product_options_active_name_idx ON commerce.product_options (store_id, product_id, name) WHERE archived_at IS NULL;
CREATE UNIQUE INDEX product_options_active_position_idx ON commerce.product_options (store_id, product_id, position) WHERE archived_at IS NULL;
CREATE UNIQUE INDEX product_option_values_active_value_idx ON commerce.product_option_values (store_id, product_id, option_id, value) WHERE archived_at IS NULL;
CREATE UNIQUE INDEX product_option_values_active_position_idx ON commerce.product_option_values (store_id, product_id, option_id, position) WHERE archived_at IS NULL;
CREATE INDEX variant_selected_options_product_idx ON commerce.variant_selected_options (store_id, product_id, variant_id, option_id);
CREATE INDEX variant_selected_options_option_value_idx ON commerce.variant_selected_options (store_id, product_id, option_id, option_value_id, variant_id);
CREATE INDEX product_publications_channel_product_idx ON commerce.product_publications (store_id, sales_channel_id, product_id);
CREATE INDEX collections_store_status_created_idx ON commerce.collections (store_id, status, created_at DESC, id DESC);
CREATE INDEX collection_products_product_idx ON commerce.collection_products (store_id, product_id, collection_id);
CREATE INDEX collection_products_collection_position_idx ON commerce.collection_products (store_id, collection_id, position, product_id);
CREATE INDEX collection_publications_channel_collection_idx ON commerce.collection_publications (store_id, sales_channel_id, collection_id);
CREATE INDEX media_assets_store_status_idx ON commerce.media_assets (store_id, status, created_at, id);
CREATE UNIQUE INDEX product_media_assets_position_active_idx ON commerce.product_media_assets (store_id, product_id, position) WHERE archived_at IS NULL;
CREATE INDEX product_media_assets_product_idx ON commerce.product_media_assets (store_id, product_id, position, media_asset_id);
CREATE INDEX product_media_assets_media_asset_idx ON commerce.product_media_assets (store_id, media_asset_id, product_id);
CREATE UNIQUE INDEX product_option_value_media_assets_position_active_idx ON commerce.product_option_value_media_assets (store_id, product_id, option_id, option_value_id, position) WHERE archived_at IS NULL;
CREATE INDEX product_option_value_media_assets_target_idx ON commerce.product_option_value_media_assets (store_id, product_id, option_id, option_value_id, position, media_asset_id);
CREATE INDEX product_option_value_media_assets_media_asset_idx ON commerce.product_option_value_media_assets (store_id, media_asset_id, product_id);
CREATE UNIQUE INDEX product_variant_media_assets_position_active_idx ON commerce.product_variant_media_assets (store_id, product_id, product_variant_id, position) WHERE archived_at IS NULL;
CREATE INDEX product_variant_media_assets_target_idx ON commerce.product_variant_media_assets (store_id, product_id, product_variant_id, position, media_asset_id);
CREATE INDEX product_variant_media_assets_media_asset_idx ON commerce.product_variant_media_assets (store_id, media_asset_id, product_id, product_variant_id);
CREATE UNIQUE INDEX product_meta_media_assets_path_active_idx ON commerce.product_meta_media_assets (store_id, product_id, meta_path) WHERE archived_at IS NULL;
CREATE INDEX product_meta_media_assets_product_idx ON commerce.product_meta_media_assets (store_id, product_id, meta_path, media_asset_id);
CREATE INDEX product_meta_media_assets_media_asset_idx ON commerce.product_meta_media_assets (store_id, media_asset_id, product_id);
CREATE INDEX reviews_product_status_idx ON commerce.reviews (store_id, product_id, status, created_at, id);
CREATE INDEX reviews_store_status_id_idx ON commerce.reviews (store_id, status, id DESC);
CREATE INDEX reviews_created_by_user_idx ON commerce.reviews (created_by_user_id, id) WHERE created_by_user_id IS NOT NULL;
CREATE INDEX reviews_parent_product_idx ON commerce.reviews (store_id, product_id, parent_review_id) WHERE parent_review_id IS NOT NULL;
CREATE INDEX reviews_parent_idx ON commerce.reviews (store_id, parent_review_id) WHERE parent_review_id IS NOT NULL;
CREATE UNIQUE INDEX review_media_assets_position_active_idx ON commerce.review_media_assets (store_id, review_id, position) WHERE archived_at IS NULL;
CREATE INDEX review_media_assets_review_idx ON commerce.review_media_assets (store_id, review_id, position, media_asset_id);
CREATE INDEX review_media_assets_media_asset_idx ON commerce.review_media_assets (store_id, media_asset_id, review_id);
CREATE INDEX product_documents_search_idx ON commerce.product_documents USING GIN (document);
CREATE INDEX price_lists_store_activation_idx ON commerce.price_lists (store_id, status, currency, starts_at, ends_at);
CREATE INDEX prices_variant_lookup_idx ON commerce.price_list_items (store_id, product_variant_id, price_list_id);

CREATE FUNCTION commerce.refresh_product_document (store_id UUID, product_id UUID)
RETURNS VOID
LANGUAGE SQL
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    INSERT INTO commerce.product_documents (store_id, product_id, document, indexed_at)
    SELECT
        product.store_id,
        product.id,
        to_tsvector('simple', concat_ws(
            ' ',
            product.handle::text,
            product.title,
            product.description,
            string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
        )),
        CURRENT_TIMESTAMP
    FROM
        commerce.products AS product
        LEFT JOIN commerce.product_variants AS variant
            ON variant.store_id = product.store_id
            AND variant.product_id = product.id
            AND variant.status = 'active'
    WHERE
        product.store_id = $1
        AND product.id = $2
    GROUP BY
        product.store_id, product.id
    ON CONFLICT (store_id, product_id)
        DO UPDATE
            SET document = EXCLUDED.document,
                indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION commerce.check_price_list_currency ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF NEW.currency <> (SELECT store.currency FROM commerce.stores AS store WHERE store.id = NEW.store_id) THEN
        RAISE EXCEPTION 'price list currency must match the store currency'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER price_lists_currency_matches_store BEFORE INSERT OR UPDATE OF currency, store_id ON commerce.price_lists FOR EACH ROW EXECUTE FUNCTION commerce.check_price_list_currency();

CREATE TRIGGER products_search_change AFTER INSERT OR UPDATE OF handle, title, description ON commerce.products FOR EACH ROW EXECUTE FUNCTION commerce.capture_product_change();
CREATE TRIGGER variants_search_change AFTER INSERT OR UPDATE OF title, sku, status OR DELETE ON commerce.product_variants FOR EACH ROW EXECUTE FUNCTION commerce.capture_variant_change();

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
ALTER TABLE commerce.product_media_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.product_option_value_media_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.product_variant_media_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.product_meta_media_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.review_media_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.product_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.price_lists ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.price_list_items ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.products
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_options
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_option_values
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_variants
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.variant_selected_options
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_publications
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.collections
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.collection_products
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.collection_publications
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_option_value_media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_variant_media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_meta_media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.reviews
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.review_media_assets
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.product_documents
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.price_lists
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.price_list_items
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION commerce.check_price_list_currency () FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.refresh_product_document (UUID, UUID) FROM PUBLIC;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.products,
       commerce.product_options,
       commerce.product_option_values,
       commerce.product_variants,
       commerce.variant_selected_options,
       commerce.product_publications,
       commerce.collections,
       commerce.collection_products,
       commerce.collection_publications,
       commerce.media_assets,
       commerce.product_media_assets,
       commerce.product_option_value_media_assets,
       commerce.product_variant_media_assets,
       commerce.product_meta_media_assets,
       commerce.reviews,
       commerce.review_media_assets,
       commerce.product_documents,
       commerce.price_lists,
       commerce.price_list_items
    TO chaos_runtime;

REVOKE DELETE ON commerce.collections, commerce.media_assets, commerce.product_media_assets,
                commerce.product_option_value_media_assets, commerce.product_variant_media_assets,
                commerce.product_meta_media_assets,
                commerce.reviews, commerce.review_media_assets FROM chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.process_events (INTEGER, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
