CREATE TYPE catalog.product_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE catalog.variant_status AS ENUM ('active', 'archived');

CREATE TYPE catalog.collection_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE catalog.collection_event_kind AS ENUM (
    'created',
    'updated',
    'activated',
    'archived',
    'products_replaced',
    'published',
    'unpublished'
);

CREATE TYPE catalog.media_kind AS ENUM ('image', 'video');

CREATE TYPE catalog.media_asset_status AS ENUM ('pending_upload', 'ready', 'archived');

CREATE TYPE catalog.media_event_kind AS ENUM ('created', 'ready', 'archived');

CREATE TYPE catalog.translation_event_kind AS ENUM ('upserted', 'removed');

CREATE TYPE catalog.review_status AS ENUM ('pending', 'approved', 'rejected');

CREATE TYPE catalog.review_event_kind AS ENUM ('submitted', 'approved', 'rejected', 'reply_added');

CREATE TYPE pricing.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE pricing.tax_rule_status AS ENUM ('active', 'archived');

CREATE TYPE pricing.promotion_status AS ENUM ('active', 'archived');

CREATE TYPE pricing.promotion_trigger AS ENUM ('automatic', 'code');

CREATE TYPE pricing.promotion_value_kind AS ENUM ('percentage', 'fixed_amount');

CREATE TYPE inventory.inventory_location_status AS ENUM ('active', 'archived');

CREATE TYPE inventory.inventory_reservation_status AS ENUM (
    'active',
    'released',
    'consumed',
    'expired'
);

CREATE TYPE inventory.stock_ledger_kind AS ENUM (
    'manual_adjustment',
    'reservation_created',
    'reservation_released',
    'reservation_consumed',
    'reservation_expired',
    'return_restock'
);

CREATE TABLE catalog.products (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               catalog.product_status     NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
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

CREATE TABLE catalog.product_translations (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    title               TEXT        NOT NULL,
    description         TEXT        NOT NULL DEFAULT '',
    updated_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, product_id, locale),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT product_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT product_translations_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT product_translations_description_length_check CHECK (
        length(description) <= 100000
    )
);

CREATE TABLE catalog.product_options (
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
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_options_name_length_check CHECK (
        length(trim(name::text)) BETWEEN 1 AND 80
    ),
    CONSTRAINT product_options_position_check CHECK (
        position BETWEEN 0 AND 9
    )
);

CREATE TABLE catalog.product_option_values (
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
        REFERENCES catalog.product_options(store_id, product_id, id)
        ON DELETE CASCADE,
    CONSTRAINT product_option_values_value_length_check CHECK (
        length(trim(value::text)) BETWEEN 1 AND 120
    ),
    CONSTRAINT product_option_values_position_check CHECK (
        position BETWEEN 0 AND 999
    )
);

CREATE TABLE catalog.product_variants (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    product_id           UUID                       NOT NULL,
    title                TEXT                       NOT NULL,
    sku                  extensions.citext,
    status               catalog.variant_status     NOT NULL DEFAULT 'active',
    requires_shipping    BOOLEAN                    NOT NULL DEFAULT true,
    track_inventory      BOOLEAN                    NOT NULL DEFAULT true,
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, product_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variants_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT product_variants_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku::text)) BETWEEN 1 AND 64
    ),
    CONSTRAINT product_variants_sku_characters_check CHECK (
        sku IS NULL OR sku::text !~ '[[:cntrl:]]'
    ),
    CONSTRAINT product_variants_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE catalog.product_variant_translations (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    product_variant_id  UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    title               TEXT        NOT NULL,
    updated_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, product_id, product_variant_id, locale),
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, locale)
        REFERENCES catalog.product_translations(store_id, product_id, locale
        ) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT product_variant_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT product_variant_translations_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    )
);

CREATE TABLE catalog.product_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    product_id          UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          catalog.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT product_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE catalog.variant_selected_options (
    store_id             UUID    NOT NULL,
    product_id           UUID    NOT NULL,
    variant_id           UUID    NOT NULL,
    option_id            UUID    NOT NULL,
    option_value_id      UUID    NOT NULL,

    PRIMARY KEY (store_id, variant_id, option_id),
    UNIQUE (store_id, variant_id, option_value_id),
    FOREIGN KEY (store_id, product_id, variant_id)
        REFERENCES catalog.product_variants(store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, option_id, option_value_id)
        REFERENCES catalog.product_option_values(store_id,
            product_id,
            option_id,
            id
        ) ON DELETE CASCADE
);

CREATE TABLE catalog.product_publications (
    store_id             UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id, sales_channel_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE catalog.collections (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               catalog.collection_status  NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
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

CREATE TABLE catalog.collection_translations (
    store_id            UUID        NOT NULL,
    collection_id       UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    title               TEXT        NOT NULL,
    description         TEXT        NOT NULL DEFAULT '',
    updated_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, collection_id, locale),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES catalog.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT collection_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT collection_translations_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT collection_translations_description_length_check CHECK (
        length(description) <= 100000
    )
);

CREATE TABLE catalog.collection_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    collection_id       UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          catalog.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, collection_id)
        REFERENCES catalog.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT collection_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE catalog.collection_products (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    position             INTEGER     NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, collection_id, product_id),
    UNIQUE (store_id, collection_id, position),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES catalog.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_products_position_check CHECK (position BETWEEN 0 AND 999)
);

CREATE TABLE catalog.collection_publications (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, collection_id, sales_channel_id),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES catalog.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE catalog.collection_events (
    id                   UUID                           NOT NULL PRIMARY KEY,
    store_id             UUID                           NOT NULL,
    collection_id        UUID                           NOT NULL,
    event_kind           catalog.collection_event_kind  NOT NULL,
    actor_user_id        UUID                           NOT NULL,
    sales_channel_id     UUID,
    product_count        INTEGER,
    occurred_at          TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, collection_id)
        REFERENCES catalog.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    CONSTRAINT collection_events_product_count_check CHECK (
        product_count IS NULL OR product_count BETWEEN 0 AND 1000
    ),
    CONSTRAINT collection_events_shape_check CHECK (
        (event_kind = 'products_replaced' AND product_count IS NOT NULL
            AND sales_channel_id IS NULL)
        OR (event_kind IN ('published', 'unpublished') AND sales_channel_id IS NOT NULL
            AND product_count IS NULL)
        OR (event_kind IN ('created', 'updated', 'activated', 'archived')
            AND sales_channel_id IS NULL AND product_count IS NULL)
    )
);

CREATE TABLE catalog.media_assets (
    id                   UUID                        NOT NULL PRIMARY KEY,
    store_id             UUID                        NOT NULL,
    product_id           UUID                        NOT NULL,
    product_variant_id   UUID,
    object_key           TEXT                        NOT NULL UNIQUE,
    file_name            TEXT                        NOT NULL,
    media_type           TEXT                        NOT NULL,
    media_kind           catalog.media_kind          NOT NULL,
    byte_size            BIGINT                      NOT NULL,
    sha256_digest        BYTEA                       NOT NULL,
    alt_text             TEXT                        NOT NULL DEFAULT '',
    position             SMALLINT                    NOT NULL,
    status               catalog.media_asset_status  NOT NULL DEFAULT 'pending_upload',
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
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, product_id, id),
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

CREATE TABLE catalog.media_asset_translations (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    media_asset_id      UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    alt_text            TEXT        NOT NULL DEFAULT '',
    updated_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, product_id, media_asset_id, locale),
    FOREIGN KEY (store_id, media_asset_id)
        REFERENCES catalog.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT media_asset_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT media_asset_translations_alt_text_check CHECK (
        length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'
    )
);

CREATE TABLE catalog.media_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    product_id          UUID                           NOT NULL,
    media_asset_id      UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          catalog.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, media_asset_id)
        REFERENCES catalog.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT media_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE catalog.media_events (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    product_id           UUID                      NOT NULL,
    media_asset_id       UUID                      NOT NULL,
    event_kind           catalog.media_event_kind  NOT NULL,
    actor_user_id        UUID                      NOT NULL,
    occurred_at          TIMESTAMPTZ               NOT NULL,

    FOREIGN KEY (store_id, media_asset_id)
        REFERENCES catalog.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id)
);

CREATE TABLE catalog.reviews (
    id                   UUID                     NOT NULL PRIMARY KEY,
    store_id             UUID                     NOT NULL,
    product_id           UUID                     NOT NULL,
    parent_review_id     UUID,
    rating               SMALLINT,
    title                TEXT,
    content              TEXT                     NOT NULL,
    author_name          TEXT                     NOT NULL,
    author_email         extensions.citext,
    status               catalog.review_status    NOT NULL DEFAULT 'pending',
    is_staff_reply       BOOLEAN                  NOT NULL DEFAULT false,
    verified_buyer       BOOLEAN                  NOT NULL DEFAULT false,
    approved_by_user_id  UUID,
    approved_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, parent_review_id)
        REFERENCES catalog.reviews(store_id, id) ON DELETE CASCADE,
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

CREATE TABLE catalog.review_events (
    id                   UUID                        NOT NULL PRIMARY KEY,
    store_id             UUID                        NOT NULL,
    review_id            UUID                        NOT NULL,
    event_kind           catalog.review_event_kind   NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id, review_id)
        REFERENCES catalog.reviews(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT review_events_actor_shape_check CHECK (
        (event_kind = 'submitted' AND actor_user_id IS NULL)
        OR (event_kind IN ('approved', 'rejected', 'reply_added') AND actor_user_id IS NOT NULL)
    )
);

CREATE TABLE pricing.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    code                 extensions.citext            NOT NULL,
    name                 TEXT                         NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    tax_inclusive        BOOLEAN                      NOT NULL DEFAULT false,
    status               pricing.price_list_status    NOT NULL DEFAULT 'draft',
    starts_at            TIMESTAMPTZ,
    ends_at              TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES merchant.store_currencies(store_id, currency),
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

CREATE TABLE pricing.prices (
    id                   UUID         NOT NULL PRIMARY KEY,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, price_list_id, product_variant_id),
    FOREIGN KEY (store_id, price_list_id)
        REFERENCES pricing.price_lists(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT prices_amount_nonnegative_check CHECK (
        amount_minor >= 0
    )
);

CREATE TABLE pricing.tax_rules (
    id                    UUID                    NOT NULL PRIMARY KEY,
    store_id              UUID                    NOT NULL,
    code                  TEXT                    NOT NULL,
    name                  TEXT                    NOT NULL,
    country_code          CHAR(2)                 NOT NULL,
    rate_basis_points     INTEGER                 NOT NULL,
    status                pricing.tax_rule_status NOT NULL DEFAULT 'active',
    created_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT tax_rules_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT tax_rules_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_rules_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT tax_rules_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE pricing.promotions (
    id                            UUID                         NOT NULL PRIMARY KEY,
    store_id                      UUID                         NOT NULL,
    handle                        TEXT                         NOT NULL,
    name                          TEXT                         NOT NULL,
    trigger                       pricing.promotion_trigger    NOT NULL,
    redemption_code               extensions.citext,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                      NOT NULL,
    minimum_subtotal_amount_minor BIGINT                       NOT NULL DEFAULT 0,
    priority                      SMALLINT                     NOT NULL DEFAULT 100,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    status                        pricing.promotion_status     NOT NULL DEFAULT 'active',
    created_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, handle),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES merchant.store_currencies(store_id, currency),
    CONSTRAINT promotions_handle_format_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT promotions_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT promotions_redemption_shape_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code::text ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT promotions_value_shape_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT promotions_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT promotions_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT promotions_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT promotions_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    )
);

ALTER TABLE pricing.price_lists
    ADD UNIQUE (store_id, id, currency);

CREATE TABLE inventory.inventory_locations (
    id                   UUID                                    NOT NULL PRIMARY KEY,
    store_id             UUID                                    NOT NULL,
    code                 extensions.citext                       NOT NULL,
    name                 TEXT                                    NOT NULL,
    status               inventory.inventory_location_status     NOT NULL DEFAULT 'active',
    created_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT inventory_locations_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT inventory_locations_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE inventory.stock_items (
    id                    UUID        NOT NULL PRIMARY KEY,
    store_id              UUID        NOT NULL,
    inventory_location_id UUID        NOT NULL,
    product_variant_id    UUID        NOT NULL,
    on_hand_quantity      BIGINT      NOT NULL DEFAULT 0,
    reserved_quantity     BIGINT      NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, inventory_location_id, product_variant_id),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, inventory_location_id)
        REFERENCES inventory.inventory_locations(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT stock_items_on_hand_nonnegative_check CHECK (on_hand_quantity >= 0),
    CONSTRAINT stock_items_reserved_range_check CHECK (
        reserved_quantity >= 0 AND reserved_quantity <= on_hand_quantity
    )
);

CREATE TABLE inventory.inventory_reservations (
    id                   UUID                                      NOT NULL PRIMARY KEY,
    store_id             UUID                                      NOT NULL,
    sales_channel_id     UUID                                      NOT NULL,
    status               inventory.inventory_reservation_status    NOT NULL DEFAULT 'active',
    expires_at           TIMESTAMPTZ                               NOT NULL,
    closed_at            TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    CONSTRAINT inventory_reservations_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT inventory_reservations_closure_check CHECK (
        (status = 'active' AND closed_at IS NULL)
        OR (status <> 'active' AND closed_at IS NOT NULL)
    )
);

CREATE TABLE inventory.inventory_reservation_lines (
    store_id             UUID    NOT NULL,
    reservation_id       UUID    NOT NULL,
    stock_item_id        UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             BIGINT  NOT NULL,

    PRIMARY KEY (store_id, reservation_id, stock_item_id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES inventory.stock_items(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT inventory_reservation_lines_quantity_positive_check CHECK (quantity > 0)
);

CREATE TABLE inventory.stock_ledger_entries (
    id                           UUID                        NOT NULL PRIMARY KEY,
    store_id                     UUID                        NOT NULL,
    stock_item_id                UUID                        NOT NULL,
    reservation_id               UUID,
    kind                         inventory.stock_ledger_kind NOT NULL,
    on_hand_delta_quantity       BIGINT                      NOT NULL,
    reserved_delta_quantity      BIGINT                      NOT NULL,
    resulting_on_hand_quantity   BIGINT                      NOT NULL,
    resulting_reserved_quantity  BIGINT                      NOT NULL,
    note                         TEXT,
    actor_user_id                UUID,
    created_at                   TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES inventory.stock_items(store_id, id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id),
    FOREIGN KEY (actor_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT stock_ledger_entries_resulting_balance_check CHECK (
        resulting_on_hand_quantity >= 0
        AND resulting_reserved_quantity >= 0
        AND resulting_reserved_quantity <= resulting_on_hand_quantity
    ),
    CONSTRAINT stock_ledger_entries_note_length_check CHECK (
        note IS NULL OR length(trim(note)) BETWEEN 1 AND 500
    ),
    CONSTRAINT stock_ledger_entries_kind_deltas_check CHECK (
        (
            kind IN ('manual_adjustment', 'return_restock')
            AND reservation_id IS NULL
            AND (
                (kind = 'manual_adjustment' AND on_hand_delta_quantity <> 0)
                OR (kind = 'return_restock' AND on_hand_delta_quantity > 0)
            )
            AND reserved_delta_quantity = 0
        )
        OR (
            kind = 'reservation_created'
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity = 0
            AND reserved_delta_quantity > 0
        )
        OR (
            kind IN ('reservation_released', 'reservation_expired')
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity = 0
            AND reserved_delta_quantity < 0
        )
        OR (
            kind = 'reservation_consumed'
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity < 0
            AND reserved_delta_quantity = on_hand_delta_quantity
        )
    )
);

CREATE TABLE search.product_documents (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    document            TSVECTOR    NOT NULL,
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE
);

CREATE INDEX products_store_status_created_idx
    ON catalog.products (store_id, status, created_at DESC, id DESC);

CREATE UNIQUE INDEX product_variants_store_sku_key
    ON catalog.product_variants (store_id, sku)
    WHERE sku IS NOT NULL;

CREATE INDEX product_variants_product_status_idx
    ON catalog.product_variants (store_id, product_id, status);

CREATE INDEX product_translation_events_product_occurred_idx
    ON catalog.product_translation_events (store_id, product_id, occurred_at, id
    );

CREATE INDEX product_publications_channel_product_idx
    ON catalog.product_publications (store_id,
        sales_channel_id,
        product_id
    );

CREATE INDEX collections_store_status_created_idx
    ON catalog.collections (store_id, status, created_at DESC, id DESC
    );

CREATE INDEX collection_translation_events_collection_occurred_idx
    ON catalog.collection_translation_events (store_id, collection_id, occurred_at, id
    );

CREATE INDEX collection_products_product_idx
    ON catalog.collection_products (store_id, product_id, collection_id);

CREATE INDEX collection_publications_channel_collection_idx
    ON catalog.collection_publications (store_id, sales_channel_id, collection_id
    );

CREATE INDEX collection_events_collection_occurred_idx
    ON catalog.collection_events (store_id, collection_id, occurred_at, id
    );

CREATE UNIQUE INDEX media_assets_product_position_active_idx
    ON catalog.media_assets (store_id, product_id, position)
    WHERE status <> 'archived';

CREATE INDEX media_assets_product_status_position_idx
    ON catalog.media_assets (store_id, product_id, status, position, id
    );

CREATE INDEX media_translation_events_asset_occurred_idx
    ON catalog.media_translation_events (store_id, media_asset_id, occurred_at, id
    );

CREATE INDEX media_events_asset_occurred_idx
    ON catalog.media_events (store_id, product_id, media_asset_id, occurred_at, id
    );

CREATE INDEX reviews_product_status_idx
    ON catalog.reviews (store_id, product_id, status, created_at, id);

CREATE INDEX reviews_parent_idx
    ON catalog.reviews (store_id, parent_review_id)
    WHERE parent_review_id IS NOT NULL;

CREATE INDEX review_events_review_occurred_idx
    ON catalog.review_events (store_id, review_id, occurred_at, id);

CREATE INDEX price_lists_store_activation_idx
    ON pricing.price_lists (store_id,
        status,
        currency,
        starts_at,
        ends_at
    );

CREATE INDEX prices_variant_lookup_idx
    ON pricing.prices (store_id,
        product_variant_id,
        price_list_id
    );

CREATE UNIQUE INDEX tax_rules_active_country_key
    ON pricing.tax_rules (store_id, country_code)
    WHERE status = 'active';

CREATE INDEX tax_rules_store_status_idx
    ON pricing.tax_rules (store_id, status, created_at, id);

CREATE UNIQUE INDEX promotions_active_redemption_code_key
    ON pricing.promotions (store_id, redemption_code)
    WHERE status = 'active' AND redemption_code IS NOT NULL;

CREATE INDEX promotions_checkout_lookup_idx
    ON pricing.promotions (store_id, currency, status, trigger, priority, id
    );

CREATE INDEX inventory_locations_store_status_idx
    ON inventory.inventory_locations (store_id, status, created_at, id);

CREATE INDEX stock_items_variant_availability_idx
    ON inventory.stock_items (store_id,
        product_variant_id,
        inventory_location_id
    );

CREATE INDEX inventory_reservations_expiration_idx
    ON inventory.inventory_reservations (store_id,
        status,
        expires_at,
        id
    );

CREATE INDEX inventory_reservation_lines_stock_item_idx
    ON inventory.inventory_reservation_lines (store_id,
        stock_item_id,
        reservation_id
    );

CREATE INDEX stock_ledger_entries_stock_item_created_idx
    ON inventory.stock_ledger_entries (store_id,
        stock_item_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX product_documents_search_idx
    ON search.product_documents USING GIN (document);

CREATE FUNCTION search.refresh_product_document(UUID, UUID)
RETURNS VOID LANGUAGE SQL SECURITY DEFINER SET search_path = pg_catalog AS $$
    INSERT INTO search.product_documents (store_id, product_id, document, indexed_at)
    SELECT product.store_id, product.id,
           to_tsvector('simple', concat_ws(
               ' ', product.handle::text, product.title, product.description,
               string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
           )), CURRENT_TIMESTAMP
      FROM catalog.products AS product
      LEFT JOIN catalog.product_variants AS variant
        ON variant.store_id = product.store_id AND variant.product_id = product.id
     WHERE product.store_id = $1 AND product.id = $2
     GROUP BY product.store_id, product.id
    ON CONFLICT (store_id, product_id) DO UPDATE
        SET document = EXCLUDED.document, indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION search.capture_product_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO integration.outbox_events (
        id, store_id, aggregate_type, aggregate_id, event_type, payload
    ) VALUES (
        uuidv7(), NEW.store_id, 'product', NEW.id,
        'search.product.changed', jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION search.capture_variant_change()
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
        SELECT 1 FROM merchant.stores
         WHERE id = owning_store_id
    ) THEN
        INSERT INTO integration.outbox_events (
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

CREATE FUNCTION search.rebuild_store_products(UUID)
RETURNS BIGINT LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE product_id UUID; rebuilt BIGINT := 0;
BEGIN
    DELETE FROM search.product_documents WHERE store_id = $1;
    FOR product_id IN SELECT id FROM catalog.products
        WHERE store_id = $1
    LOOP
        PERFORM search.refresh_product_document($1, product_id);
        rebuilt := rebuilt + 1;
    END LOOP;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION search.process_events(UUID, INTEGER, TIMESTAMPTZ)
RETURNS BIGINT LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE event RECORD; processed BIGINT := 0;
BEGIN
    FOR event IN
        SELECT outbox.id, outbox.store_id, outbox.aggregate_id
          FROM integration.outbox_events AS outbox
          INNER JOIN integration.event_consumer_registry AS registry
            ON registry.event_type = outbox.event_type
           AND registry.consumer_owner = 'search.product_indexer'
         WHERE outbox.status = 'pending' AND outbox.event_type = 'search.product.changed'
           AND outbox.available_at <= $3
         ORDER BY outbox.available_at, outbox.created_at, outbox.id
         FOR UPDATE OF outbox SKIP LOCKED
         LIMIT greatest(least($2, 100), 1)
    LOOP
        UPDATE integration.outbox_events
           SET status = 'processing', attempts = attempts + 1,
               locked_by = $1, locked_at = $3
         WHERE id = event.id;
        PERFORM search.refresh_product_document(
            event.store_id, event.aggregate_id
        );
        UPDATE integration.outbox_events
           SET status = 'processed', processed_at = $3,
               locked_by = NULL, locked_at = NULL
         WHERE id = event.id AND locked_by = $1;
        processed := processed + 1;
    END LOOP;
    RETURN processed;
END;
$$;

CREATE TRIGGER products_search_change
AFTER INSERT OR UPDATE OF handle, title, description ON catalog.products
FOR EACH ROW EXECUTE FUNCTION search.capture_product_change();

CREATE TRIGGER variants_search_change
AFTER INSERT OR UPDATE OF title, sku OR DELETE ON catalog.product_variants
FOR EACH ROW EXECUTE FUNCTION search.capture_variant_change();

ALTER TABLE catalog.products ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_option_values ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_variants ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_variant_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.variant_selected_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.product_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collections ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collection_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collection_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collection_products ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collection_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.collection_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.media_assets ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.media_asset_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.media_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.media_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.reviews ENABLE ROW LEVEL SECURITY;

ALTER TABLE catalog.review_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.price_lists ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.prices ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.tax_rules ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.promotions ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.inventory_locations ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.stock_items ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.inventory_reservations ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.inventory_reservation_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.stock_ledger_entries ENABLE ROW LEVEL SECURITY;

ALTER TABLE search.product_documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON catalog.products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_option_values
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_variants
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_variant_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_translation_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.variant_selected_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.product_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collection_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collection_translation_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collection_products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collection_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.collection_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.media_assets
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.media_asset_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.media_translation_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.media_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.reviews
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON catalog.review_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.price_lists
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.prices
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.tax_rules
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.promotions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.inventory_locations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.stock_items
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.inventory_reservations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.inventory_reservation_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.stock_ledger_entries
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON search.product_documents
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA catalog TO chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.collection_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.collection_translation_events FROM chaos_runtime;

REVOKE DELETE ON catalog.collections FROM chaos_runtime;

REVOKE DELETE ON catalog.media_assets FROM chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.media_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.media_translation_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.product_translation_events FROM chaos_runtime;

REVOKE DELETE ON catalog.reviews FROM chaos_runtime;

REVOKE UPDATE, DELETE ON catalog.review_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA catalog TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA catalog
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA catalog
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA pricing TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA pricing TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA inventory TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON inventory.stock_ledger_entries FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA inventory TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT ON ALL TABLES IN SCHEMA search TO chaos_runtime;

GRANT EXECUTE ON FUNCTION search.rebuild_store_products(UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION search.process_events(UUID, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA search
    GRANT SELECT ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA catalog, pricing, inventory, search TO chaos_runtime;
