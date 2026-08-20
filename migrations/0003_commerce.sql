-- === Store foundation ===

CREATE SCHEMA commerce;

COMMENT ON SCHEMA commerce IS
    'Store-owned commerce data and Storefront read models';

CREATE TYPE commerce.store_role AS ENUM ('owner', 'member');

CREATE TYPE commerce.store_status AS ENUM ('active', 'inactive');

CREATE TYPE commerce.sales_channel_kind AS ENUM (
    'web',
    'mobile',
    'point_of_sale',
    'marketplace',
    'custom'
);

CREATE TYPE commerce.sales_channel_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.store_locale_event_kind AS ENUM ('enabled', 'disabled', 'default_changed');

CREATE TYPE commerce.publishable_key_scope AS ENUM (
    'analytics:write',
    'catalog:read',
    'carts:write',
    'checkout:write',
    'orders:read',
    'reviews:write'
);

CREATE TABLE commerce.stores (
    id                   UUID                     NOT NULL PRIMARY KEY,
    code                 extensions.citext        NOT NULL UNIQUE,
    name                 TEXT                     NOT NULL,
    default_region       CHAR(2)                  NOT NULL DEFAULT 'US',
    default_currency     CHAR(3)                  NOT NULL DEFAULT 'USD',
    default_locale       VARCHAR(63)              NOT NULL DEFAULT 'en-US',
    status               commerce.store_status    NOT NULL DEFAULT 'inactive',
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

CREATE TABLE commerce.store_locale_events (
    id                  UUID                             NOT NULL PRIMARY KEY,
    store_id            UUID                             NOT NULL,
    locale              VARCHAR(63)                      NOT NULL,
    previous_locale     VARCHAR(63),
    event_kind          commerce.store_locale_event_kind NOT NULL,
    actor_user_id       UUID                             NOT NULL,
    occurred_at         TIMESTAMPTZ                      NOT NULL,

    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT store_locale_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
        AND (previous_locale IS NULL
            OR previous_locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$')
    ),
    CONSTRAINT store_locale_events_shape_check CHECK (
        (event_kind = 'default_changed' AND previous_locale IS NOT NULL)
        OR (event_kind <> 'default_changed' AND previous_locale IS NULL)
    )
);

CREATE TABLE commerce.sales_channels (
    id                   UUID                              NOT NULL PRIMARY KEY,
    store_id             UUID                              NOT NULL,
    code                 extensions.citext                 NOT NULL,
    name                 TEXT                              NOT NULL,
    kind                 commerce.sales_channel_kind       NOT NULL,
    status               commerce.sales_channel_status     NOT NULL DEFAULT 'active',
    is_default           BOOLEAN                           NOT NULL DEFAULT false,
    created_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT sales_channels_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT sales_channels_name_length_check CHECK (
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

CREATE TABLE commerce.publishable_keys (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    sales_channel_id     UUID,
    key_identifier       TEXT                      NOT NULL UNIQUE,
    secret_digest        BYTEA                     NOT NULL,
    display_suffix       CHAR(4)                   NOT NULL,
    name                 TEXT                      NOT NULL,
    created_by_user_id   UUID                      NOT NULL,
    revoked_by_user_id   UUID,
    expires_at           TIMESTAMPTZ,
    last_used_at         TIMESTAMPTZ,
    revoked_at           TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
    FOREIGN KEY (created_by_user_id)
        REFERENCES identity.users(id),
    FOREIGN KEY (revoked_by_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT publishable_keys_identifier_format_check CHECK (
        key_identifier ~ '^[A-Za-z0-9_-]{16}$'
    ),
    CONSTRAINT publishable_keys_secret_digest_length_check CHECK (
        octet_length(secret_digest) = 32
    ),
    CONSTRAINT publishable_keys_display_suffix_format_check CHECK (
        display_suffix ~ '^[A-Za-z0-9_-]{4}$'
    ),
    CONSTRAINT publishable_keys_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    ),
    CONSTRAINT publishable_keys_expiration_check CHECK (
        expires_at IS NULL OR expires_at > created_at
    ),
    CONSTRAINT publishable_keys_revocation_check CHECK (
        (revoked_at IS NULL AND revoked_by_user_id IS NULL)
        OR (revoked_at IS NOT NULL AND revoked_by_user_id IS NOT NULL)
    )
);

CREATE TABLE commerce.publishable_key_scopes (
    publishable_key_id           UUID                      NOT NULL,
    scope                commerce.publishable_key_scope    NOT NULL,

    PRIMARY KEY (publishable_key_id, scope),
    FOREIGN KEY (publishable_key_id)
        REFERENCES commerce.publishable_keys(id) ON DELETE CASCADE,
    CONSTRAINT publishable_key_scopes_publishable_scope_check CHECK (
        scope IN (
            'analytics:write',
            'catalog:read',
            'carts:write',
            'checkout:write',
            'orders:read',
            'reviews:write'
        )
    )
);

CREATE INDEX store_memberships_user_idx
    ON commerce.store_memberships (user_id, store_id);

CREATE INDEX stores_status_idx
    ON commerce.stores (status);

CREATE INDEX store_locale_events_store_occurred_idx
    ON commerce.store_locale_events (store_id, occurred_at, id);

CREATE UNIQUE INDEX sales_channels_one_default_per_store_idx
    ON commerce.sales_channels (store_id)
    WHERE is_default;

CREATE INDEX sales_channels_store_status_idx
    ON commerce.sales_channels (store_id, status);

CREATE INDEX publishable_keys_store_created_idx
    ON commerce.publishable_keys (store_id, created_at DESC, id DESC);

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

CREATE FUNCTION commerce.authenticate_publishable_key(
    presented_key_identifier  TEXT,
    presented_secret_digest  BYTEA
)
RETURNS TABLE (
    publishable_key_id           UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    scopes               TEXT[],
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
           ARRAY(
               SELECT publishable_key_scope.scope::TEXT
               FROM commerce.publishable_key_scopes AS publishable_key_scope
               WHERE publishable_key_scope.publishable_key_id = publishable_key.id
               ORDER BY publishable_key_scope.scope::TEXT
           ),
           publishable_key.created_by_user_id
    FROM commerce.publishable_keys AS publishable_key
    INNER JOIN commerce.stores AS store
        ON store.id = publishable_key.store_id
    LEFT JOIN commerce.sales_channels AS sales_channel
        ON sales_channel.store_id = publishable_key.store_id
       AND sales_channel.id = COALESCE(
           publishable_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM commerce.sales_channels AS default_channel
               WHERE default_channel.store_id = publishable_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE publishable_key.key_identifier = presented_key_identifier
      AND publishable_key.secret_digest = presented_secret_digest
      AND publishable_key.revoked_at IS NULL
      AND (publishable_key.expires_at IS NULL OR publishable_key.expires_at > CURRENT_TIMESTAMP)
      AND store.status = 'active';
$$;

CREATE TRIGGER store_locales_protect_default
BEFORE DELETE ON commerce.store_locales
FOR EACH ROW EXECUTE FUNCTION commerce.prevent_default_locale_removal();

ALTER TABLE commerce.stores ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_memberships ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_locales ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_locale_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.store_currencies ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.sales_channels ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.publishable_keys ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.publishable_key_scopes ENABLE ROW LEVEL SECURITY;

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

CREATE POLICY store_isolation ON commerce.store_locale_events
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

CREATE POLICY store_isolation ON commerce.sales_channels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.publishable_keys
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.publishable_key_scopes
    USING (
        EXISTS (
            SELECT 1
            FROM commerce.publishable_keys AS publishable_key
            WHERE publishable_key.id = publishable_key_scopes.publishable_key_id
              AND publishable_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    )
    WITH CHECK (
        EXISTS (
            SELECT 1
            FROM commerce.publishable_keys AS publishable_key
            WHERE publishable_key.id = publishable_key_scopes.publishable_key_id
              AND publishable_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    );

REVOKE ALL ON FUNCTION commerce.authenticate_publishable_key(TEXT, BYTEA) FROM PUBLIC;

COMMENT ON FUNCTION commerce.authenticate_publishable_key(TEXT, BYTEA) IS
    'Authenticates a machine credential without exposing stored secret digests';

GRANT EXECUTE
    ON FUNCTION commerce.authenticate_publishable_key(TEXT, BYTEA) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.store_locale_events FROM chaos_runtime;

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

CREATE TYPE commerce.collection_event_kind AS ENUM (
    'created',
    'updated',
    'activated',
    'archived',
    'products_replaced',
    'published',
    'unpublished'
);

CREATE TYPE commerce.media_kind AS ENUM ('image', 'video');

CREATE TYPE commerce.media_asset_status AS ENUM ('pending_upload', 'ready', 'archived');

CREATE TYPE commerce.media_event_kind AS ENUM ('created', 'ready', 'archived');

CREATE TYPE commerce.translation_event_kind AS ENUM ('upserted', 'removed');

CREATE TYPE commerce.review_status AS ENUM ('pending', 'approved', 'rejected');

CREATE TYPE commerce.review_event_kind AS ENUM ('submitted', 'approved', 'rejected', 'reply_added');

CREATE TABLE commerce.products (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               commerce.product_status     NOT NULL DEFAULT 'draft',
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

CREATE TABLE commerce.product_translations (
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
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
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
    status               commerce.variant_status     NOT NULL DEFAULT 'active',
    requires_shipping    BOOLEAN                    NOT NULL DEFAULT true,
    track_inventory      BOOLEAN                    NOT NULL DEFAULT true,
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
    CONSTRAINT product_variants_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.product_variant_translations (
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
        REFERENCES commerce.product_variants(store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, locale)
        REFERENCES commerce.product_translations(store_id, product_id, locale
        ) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT product_variant_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT product_variant_translations_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    )
);

CREATE TABLE commerce.product_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    product_id          UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          commerce.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT product_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
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
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE
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

CREATE TABLE commerce.collection_translations (
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
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
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

CREATE TABLE commerce.collection_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    collection_id       UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          commerce.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT collection_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
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
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.collection_events (
    id                   UUID                           NOT NULL PRIMARY KEY,
    store_id             UUID                           NOT NULL,
    collection_id        UUID                           NOT NULL,
    event_kind           commerce.collection_event_kind  NOT NULL,
    actor_user_id        UUID                           NOT NULL,
    sales_channel_id     UUID,
    product_count        INTEGER,
    occurred_at          TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
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

CREATE TABLE commerce.media_asset_translations (
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
        REFERENCES commerce.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (updated_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT media_asset_translations_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT media_asset_translations_alt_text_check CHECK (
        length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'
    )
);

CREATE TABLE commerce.media_translation_events (
    id                  UUID                           NOT NULL PRIMARY KEY,
    store_id            UUID                           NOT NULL,
    product_id          UUID                           NOT NULL,
    media_asset_id      UUID                           NOT NULL,
    locale              VARCHAR(63)                    NOT NULL,
    event_kind          commerce.translation_event_kind NOT NULL,
    actor_user_id       UUID                           NOT NULL,
    occurred_at         TIMESTAMPTZ                    NOT NULL,

    FOREIGN KEY (store_id, media_asset_id)
        REFERENCES commerce.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT media_translation_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE commerce.media_events (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    product_id           UUID                      NOT NULL,
    media_asset_id       UUID                      NOT NULL,
    event_kind           commerce.media_event_kind  NOT NULL,
    actor_user_id        UUID                      NOT NULL,
    occurred_at          TIMESTAMPTZ               NOT NULL,

    FOREIGN KEY (store_id, media_asset_id)
        REFERENCES commerce.media_assets(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id)
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
    status               commerce.review_status    NOT NULL DEFAULT 'pending',
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

CREATE TABLE commerce.review_events (
    id                   UUID                        NOT NULL PRIMARY KEY,
    store_id             UUID                        NOT NULL,
    review_id            UUID                        NOT NULL,
    event_kind           commerce.review_event_kind   NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id, review_id)
        REFERENCES commerce.reviews(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT review_events_actor_shape_check CHECK (
        (event_kind = 'submitted' AND actor_user_id IS NULL)
        OR (event_kind IN ('approved', 'rejected', 'reply_added') AND actor_user_id IS NOT NULL)
    )
);

CREATE INDEX products_store_status_created_idx
    ON commerce.products (store_id, status, created_at DESC, id DESC);

CREATE UNIQUE INDEX product_variants_store_sku_key
    ON commerce.product_variants (store_id, sku)
    WHERE sku IS NOT NULL;

CREATE INDEX product_variants_product_status_idx
    ON commerce.product_variants (store_id, product_id, status);

CREATE INDEX product_translation_events_product_occurred_idx
    ON commerce.product_translation_events (store_id, product_id, occurred_at, id
    );

CREATE INDEX product_publications_channel_product_idx
    ON commerce.product_publications (store_id,
        sales_channel_id,
        product_id
    );

CREATE INDEX collections_store_status_created_idx
    ON commerce.collections (store_id, status, created_at DESC, id DESC
    );

CREATE INDEX collection_translation_events_collection_occurred_idx
    ON commerce.collection_translation_events (store_id, collection_id, occurred_at, id
    );

CREATE INDEX collection_products_product_idx
    ON commerce.collection_products (store_id, product_id, collection_id);

CREATE INDEX collection_publications_channel_collection_idx
    ON commerce.collection_publications (store_id, sales_channel_id, collection_id
    );

CREATE INDEX collection_events_collection_occurred_idx
    ON commerce.collection_events (store_id, collection_id, occurred_at, id
    );

CREATE UNIQUE INDEX media_assets_product_position_active_idx
    ON commerce.media_assets (store_id, product_id, position)
    WHERE status <> 'archived';

CREATE INDEX media_assets_product_status_position_idx
    ON commerce.media_assets (store_id, product_id, status, position, id
    );

CREATE INDEX media_translation_events_asset_occurred_idx
    ON commerce.media_translation_events (store_id, media_asset_id, occurred_at, id
    );

CREATE INDEX media_events_asset_occurred_idx
    ON commerce.media_events (store_id, product_id, media_asset_id, occurred_at, id
    );

CREATE INDEX reviews_product_status_idx
    ON commerce.reviews (store_id, product_id, status, created_at, id);

CREATE INDEX reviews_parent_idx
    ON commerce.reviews (store_id, parent_review_id)
    WHERE parent_review_id IS NOT NULL;

CREATE INDEX review_events_review_occurred_idx
    ON commerce.review_events (store_id, review_id, occurred_at, id);

ALTER TABLE commerce.products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_option_values ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_variants ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_variant_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.variant_selected_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collections ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_assets ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_asset_translations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_translation_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.reviews ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.review_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_translations
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

CREATE POLICY store_isolation ON commerce.product_variant_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_translation_events
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

CREATE POLICY store_isolation ON commerce.collection_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collection_translation_events
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

CREATE POLICY store_isolation ON commerce.collection_events
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

CREATE POLICY store_isolation ON commerce.media_asset_translations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.media_translation_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.media_events
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

CREATE POLICY store_isolation ON commerce.review_events
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

REVOKE UPDATE, DELETE ON commerce.collection_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.collection_translation_events FROM chaos_runtime;

REVOKE DELETE ON commerce.collections FROM chaos_runtime;

REVOKE DELETE ON commerce.media_assets FROM chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.media_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.media_translation_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.product_translation_events FROM chaos_runtime;

REVOKE DELETE ON commerce.reviews FROM chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.review_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Pricing ===

CREATE TYPE commerce.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE commerce.tax_rule_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.promotion_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.promotion_trigger AS ENUM ('automatic', 'code');

CREATE TYPE commerce.promotion_value_kind AS ENUM ('percentage', 'fixed_amount');

CREATE TABLE commerce.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    code                 extensions.citext            NOT NULL,
    name                 TEXT                         NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    tax_inclusive        BOOLEAN                      NOT NULL DEFAULT false,
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

CREATE TABLE commerce.tax_rules (
    id                    UUID                    NOT NULL PRIMARY KEY,
    store_id              UUID                    NOT NULL,
    code                  TEXT                    NOT NULL,
    name                  TEXT                    NOT NULL,
    country_code          CHAR(2)                 NOT NULL,
    rate_basis_points     INTEGER                 NOT NULL,
    status                commerce.tax_rule_status NOT NULL DEFAULT 'active',
    created_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT tax_rules_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT tax_rules_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_rules_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT tax_rules_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE commerce.promotions (
    id                            UUID                         NOT NULL PRIMARY KEY,
    store_id                      UUID                         NOT NULL,
    handle                        TEXT                         NOT NULL,
    name                          TEXT                         NOT NULL,
    trigger                       commerce.promotion_trigger    NOT NULL,
    redemption_code               extensions.citext,
    value_kind                    commerce.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                      NOT NULL,
    minimum_subtotal_amount_minor BIGINT                       NOT NULL DEFAULT 0,
    priority                      SMALLINT                     NOT NULL DEFAULT 100,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    status                        commerce.promotion_status     NOT NULL DEFAULT 'active',
    created_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, handle),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES commerce.store_currencies(store_id, currency),
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

CREATE UNIQUE INDEX tax_rules_active_country_key
    ON commerce.tax_rules (store_id, country_code)
    WHERE status = 'active';

CREATE INDEX tax_rules_store_status_idx
    ON commerce.tax_rules (store_id, status, created_at, id);

CREATE UNIQUE INDEX promotions_active_redemption_code_key
    ON commerce.promotions (store_id, redemption_code)
    WHERE status = 'active' AND redemption_code IS NOT NULL;

CREATE INDEX promotions_checkout_lookup_idx
    ON commerce.promotions (store_id, currency, status, trigger, priority, id
    );

ALTER TABLE commerce.price_lists ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.prices ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.tax_rules ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.promotions ENABLE ROW LEVEL SECURITY;

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

CREATE POLICY store_isolation ON commerce.tax_rules
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.promotions
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

-- === Inventory ===

CREATE TYPE commerce.inventory_location_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.inventory_reservation_status AS ENUM (
    'active',
    'released',
    'consumed',
    'expired'
);

CREATE TYPE commerce.stock_ledger_kind AS ENUM (
    'manual_adjustment',
    'reservation_created',
    'reservation_released',
    'reservation_consumed',
    'reservation_expired',
    'return_restock'
);

CREATE TABLE commerce.inventory_locations (
    id                   UUID                                    NOT NULL PRIMARY KEY,
    store_id             UUID                                    NOT NULL,
    code                 extensions.citext                       NOT NULL,
    name                 TEXT                                    NOT NULL,
    status               commerce.inventory_location_status      NOT NULL DEFAULT 'active',
    created_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT inventory_locations_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT inventory_locations_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE commerce.stock_items (
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
        REFERENCES commerce.inventory_locations(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, id),
    CONSTRAINT stock_items_on_hand_nonnegative_check CHECK (on_hand_quantity >= 0),
    CONSTRAINT stock_items_reserved_range_check CHECK (
        reserved_quantity >= 0 AND reserved_quantity <= on_hand_quantity
    )
);

CREATE TABLE commerce.inventory_reservations (
    id                   UUID                                      NOT NULL PRIMARY KEY,
    store_id             UUID                                      NOT NULL,
    sales_channel_id     UUID                                      NOT NULL,
    status               commerce.inventory_reservation_status    NOT NULL DEFAULT 'active',
    expires_at           TIMESTAMPTZ                               NOT NULL,
    closed_at            TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
    CONSTRAINT inventory_reservations_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT inventory_reservations_closure_check CHECK (
        (status = 'active' AND closed_at IS NULL)
        OR (status <> 'active' AND closed_at IS NOT NULL)
    )
);

CREATE TABLE commerce.inventory_reservation_lines (
    store_id             UUID    NOT NULL,
    reservation_id       UUID    NOT NULL,
    stock_item_id        UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             BIGINT  NOT NULL,

    PRIMARY KEY (store_id, reservation_id, stock_item_id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES commerce.inventory_reservations(store_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES commerce.stock_items(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, id),
    CONSTRAINT inventory_reservation_lines_quantity_positive_check CHECK (quantity > 0)
);

CREATE TABLE commerce.stock_ledger_entries (
    id                           UUID                        NOT NULL PRIMARY KEY,
    store_id                     UUID                        NOT NULL,
    stock_item_id                UUID                        NOT NULL,
    reservation_id               UUID,
    kind                         commerce.stock_ledger_kind NOT NULL,
    on_hand_delta_quantity       BIGINT                      NOT NULL,
    reserved_delta_quantity      BIGINT                      NOT NULL,
    resulting_on_hand_quantity   BIGINT                      NOT NULL,
    resulting_reserved_quantity  BIGINT                      NOT NULL,
    note                         TEXT,
    actor_user_id                UUID,
    created_at                   TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES commerce.stock_items(store_id, id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES commerce.inventory_reservations(store_id, id),
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

CREATE INDEX inventory_locations_store_status_idx
    ON commerce.inventory_locations (store_id, status, created_at, id);

CREATE INDEX stock_items_variant_availability_idx
    ON commerce.stock_items (store_id,
        product_variant_id,
        inventory_location_id
    );

CREATE INDEX inventory_reservations_expiration_idx
    ON commerce.inventory_reservations (store_id,
        status,
        expires_at,
        id
    );

CREATE INDEX inventory_reservation_lines_stock_item_idx
    ON commerce.inventory_reservation_lines (store_id,
        stock_item_id,
        reservation_id
    );

CREATE INDEX stock_ledger_entries_stock_item_created_idx
    ON commerce.stock_ledger_entries (store_id,
        stock_item_id,
        created_at DESC,
        id DESC
    );

ALTER TABLE commerce.inventory_locations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.stock_items ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.inventory_reservations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.inventory_reservation_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.stock_ledger_entries ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.inventory_locations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.stock_items
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.inventory_reservations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.inventory_reservation_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.stock_ledger_entries
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

REVOKE UPDATE, DELETE
    ON commerce.stock_ledger_entries FROM chaos_runtime;

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
    INSERT INTO integration.outbox_events (
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

CREATE FUNCTION commerce.process_events(UUID, INTEGER, TIMESTAMPTZ)
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
        PERFORM commerce.refresh_product_document(
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

GRANT EXECUTE ON FUNCTION commerce.process_events(UUID, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

-- === Sales, payments, and fulfillment ===

CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');

CREATE TYPE commerce.checkout_status AS ENUM ('pending', 'completed', 'expired');

CREATE TYPE commerce.address_kind AS ENUM ('billing', 'shipping');

CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_fulfillment_status AS ENUM (
    'unfulfilled',
    'partially_fulfilled',
    'fulfilled'
);

CREATE TYPE commerce.order_delivery_status AS ENUM (
    'not_delivered',
    'partially_delivered',
    'delivered'
);

CREATE TYPE commerce.shipping_service_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.payment_attempt_status AS ENUM (
    'pending',
    'authorized',
    'captured',
    'failed',
    'cancelled'
);

CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');

CREATE TYPE commerce.fulfillment_status AS ENUM (
    'pending',
    'shipped',
    'delivered',
    'cancelled'
);

CREATE TYPE commerce.return_status AS ENUM (
    'requested',
    'authorized',
    'received',
    'completed',
    'rejected'
);

CREATE TYPE commerce.return_disposition AS ENUM ('restock', 'discard');

CREATE TABLE commerce.customers (
    id                  UUID              NOT NULL PRIMARY KEY,
    store_id            UUID              NOT NULL,
    user_id             UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,
    created_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, user_id),
    UNIQUE (store_id, email),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES identity.users(id),
    CONSTRAINT customers_email_length_check CHECK (length(trim(email::text)) BETWEEN 3 AND 320),
    CONSTRAINT customers_phone_format_check CHECK (phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$')
);

CREATE TABLE commerce.customer_addresses (
    id                   UUID     NOT NULL PRIMARY KEY,
    store_id             UUID     NOT NULL,
    customer_id          UUID     NOT NULL,
    label                TEXT     NOT NULL,
    full_name            TEXT     NOT NULL,
    company              TEXT,
    address_line1        TEXT     NOT NULL,
    address_line2        TEXT,
    locality             TEXT     NOT NULL,
    administrative_area  TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)  NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, customer_id, id),
    CONSTRAINT customer_addresses_customer_label_key
        UNIQUE (store_id, customer_id, label),
    CONSTRAINT customer_addresses_customer_fkey
        FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_addresses_label_length_check CHECK (
        length(trim(label)) BETWEEN 1 AND 64
    ),
    CONSTRAINT customer_addresses_full_name_length_check CHECK (
        length(trim(full_name)) BETWEEN 1 AND 200
    ),
    CONSTRAINT customer_addresses_company_length_check CHECK (
        company IS NULL OR length(trim(company)) BETWEEN 1 AND 200
    ),
    CONSTRAINT customer_addresses_line1_length_check CHECK (
        length(trim(address_line1)) BETWEEN 1 AND 255
    ),
    CONSTRAINT customer_addresses_line2_length_check CHECK (
        address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255
    ),
    CONSTRAINT customer_addresses_locality_length_check CHECK (
        length(trim(locality)) BETWEEN 1 AND 100
    ),
    CONSTRAINT customer_addresses_area_length_check CHECK (
        administrative_area IS NULL
        OR length(trim(administrative_area)) BETWEEN 1 AND 100
    ),
    CONSTRAINT customer_addresses_postal_code_length_check CHECK (
        postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT customer_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE commerce.customer_shopper_links (
    store_id            UUID        NOT NULL,
    customer_id         UUID        NOT NULL,
    shopper_id          UUID        NOT NULL,
    sales_channel_id    UUID        NOT NULL,
    linked_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, shopper_id),
    CONSTRAINT customer_shopper_links_customer_fkey
        FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_shopper_links_channel_fkey
        FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.carts (
    id                   UUID                NOT NULL PRIMARY KEY,
    store_id             UUID                NOT NULL,
    sales_channel_id     UUID                NOT NULL,
    shopper_id           UUID                NOT NULL,
    customer_id          UUID,
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
        REFERENCES commerce.sales_channels(id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id),
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
    tax_inclusive           BOOLEAN     NOT NULL,
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

CREATE TABLE commerce.checkouts (
    id                     UUID                    NOT NULL PRIMARY KEY,
    store_id               UUID                    NOT NULL,
    cart_id                UUID                    NOT NULL,
    shopper_id             UUID                    NOT NULL,
    customer_id            UUID,
    sales_channel_id       UUID                    NOT NULL,
    price_list_id          UUID                    NOT NULL,
    inventory_reservation_id UUID,
    currency               CHAR(3)                 NOT NULL,
    locale                 VARCHAR(63)             NOT NULL DEFAULT 'en-US',
    status                 commerce.checkout_status   NOT NULL DEFAULT 'pending',
    subtotal_amount_minor  BIGINT                  NOT NULL,
    discount_amount_minor  BIGINT                  NOT NULL,
    tax_amount_minor       BIGINT                  NOT NULL,
    tax_inclusive          BOOLEAN                 NOT NULL,
    shipping_amount_minor  BIGINT                  NOT NULL,
    total_amount_minor     BIGINT                  NOT NULL,
    expires_at             TIMESTAMPTZ             NOT NULL,
    closed_at              TIMESTAMPTZ,
    expiry_locked_by       UUID,
    expiry_locked_at       TIMESTAMPTZ,
    created_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, cart_id),
    UNIQUE (store_id, inventory_reservation_id),
    FOREIGN KEY (store_id, cart_id, shopper_id)
        REFERENCES commerce.carts(store_id, id, shopper_id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    FOREIGN KEY (store_id, inventory_reservation_id)
        REFERENCES commerce.inventory_reservations(store_id, id),
    CONSTRAINT checkouts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT checkouts_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT checkouts_amounts_check CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
            + shipping_amount_minor
    ),
    CONSTRAINT checkouts_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT checkouts_closure_check CHECK (
        (status = 'pending' AND closed_at IS NULL)
        OR (status <> 'pending' AND closed_at IS NOT NULL)
    ),
    CONSTRAINT checkouts_expiry_lease_shape_check CHECK (
        (expiry_locked_by IS NULL) = (expiry_locked_at IS NULL)
        AND (status = 'pending' OR expiry_locked_by IS NULL)
    )
);

CREATE TABLE commerce.checkout_contacts (
    store_id            UUID              NOT NULL,
    checkout_id         UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id) ON DELETE CASCADE,
    CONSTRAINT checkout_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT checkout_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE TABLE commerce.checkout_addresses (
    store_id             UUID               NOT NULL,
    checkout_id          UUID               NOT NULL,
    kind                 commerce.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (store_id, checkout_id, kind),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id) ON DELETE CASCADE,
    CONSTRAINT checkout_addresses_full_name_length_check CHECK (
        length(trim(full_name)) BETWEEN 1 AND 200
    ),
    CONSTRAINT checkout_addresses_company_length_check CHECK (
        company IS NULL OR length(trim(company)) BETWEEN 1 AND 200
    ),
    CONSTRAINT checkout_addresses_line1_length_check CHECK (
        length(trim(address_line1)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_addresses_line2_length_check CHECK (
        address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_addresses_locality_length_check CHECK (
        length(trim(locality)) BETWEEN 1 AND 100
    ),
    CONSTRAINT checkout_addresses_area_length_check CHECK (
        administrative_area IS NULL
        OR length(trim(administrative_area)) BETWEEN 1 AND 100
    ),
    CONSTRAINT checkout_addresses_postal_code_length_check CHECK (
        postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT checkout_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE commerce.checkout_tax_calculations (
    store_id            UUID    NOT NULL,
    checkout_id         UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, tax_rule_id)
        REFERENCES commerce.tax_rules(store_id, id),
    CONSTRAINT checkout_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT checkout_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE commerce.checkout_promotion_calculations (
    store_id                      UUID                      NOT NULL,
    checkout_id                   UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       commerce.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    commerce.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, promotion_id)
        REFERENCES commerce.promotions(store_id, id),
    CONSTRAINT checkout_promotion_handle_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT checkout_promotion_name_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_promotion_trigger_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT checkout_promotion_value_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT checkout_promotion_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT checkout_promotion_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT checkout_promotion_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    ),
    CONSTRAINT checkout_promotion_discount_check CHECK (discount_amount_minor > 0),
    CONSTRAINT checkout_promotion_currency_check CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE commerce.checkout_lines (
    store_id                 UUID        NOT NULL,
    checkout_id              UUID        NOT NULL,
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
    discount_amount_minor    BIGINT      NOT NULL,
    tax_amount_minor         BIGINT      NOT NULL,
    total_amount_minor       BIGINT      NOT NULL,
    tax_inclusive            BOOLEAN     NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, checkout_id, position),
    UNIQUE (store_id, checkout_id, product_variant_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id),
    CONSTRAINT checkout_lines_position_check CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT checkout_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT checkout_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT checkout_lines_amounts_check CHECK (
        unit_price_amount_minor >= 0
        AND subtotal_amount_minor = unit_price_amount_minor * quantity
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
        AND (NOT tax_inclusive OR tax_amount_minor <= subtotal_amount_minor - discount_amount_minor)
    )
);

CREATE TABLE commerce.orders (
    id                       UUID                               NOT NULL PRIMARY KEY,
    store_id                 UUID                               NOT NULL,
    order_number             TEXT                               NOT NULL,
    sales_channel_id         UUID                               NOT NULL,
    checkout_id              UUID                               NOT NULL,
    shopper_id               UUID                               NOT NULL,
    customer_id              UUID,
    inventory_reservation_id UUID,
    price_list_id            UUID                               NOT NULL,
    currency                 CHAR(3)                            NOT NULL,
    locale                   VARCHAR(63)                        NOT NULL DEFAULT 'en-US',
    status                   commerce.order_status              NOT NULL DEFAULT 'pending',
    fulfillment_status       commerce.order_fulfillment_status  NOT NULL DEFAULT 'unfulfilled',
    delivery_status          commerce.order_delivery_status     NOT NULL DEFAULT 'not_delivered',
    subtotal_amount_minor    BIGINT                             NOT NULL,
    discount_amount_minor    BIGINT                             NOT NULL,
    tax_amount_minor         BIGINT                             NOT NULL,
    tax_inclusive            BOOLEAN                            NOT NULL,
    shipping_amount_minor    BIGINT                             NOT NULL,
    total_amount_minor       BIGINT                             NOT NULL,
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, order_number),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id, shopper_id)
        REFERENCES commerce.checkouts(store_id, id, shopper_id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    FOREIGN KEY (store_id, inventory_reservation_id)
        REFERENCES commerce.inventory_reservations(store_id, id),
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
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
            + shipping_amount_minor
    )
);

CREATE TABLE commerce.order_contacts (
    store_id            UUID              NOT NULL,
    order_id            UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT order_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE TABLE commerce.order_tracking_keys (
    id                UUID        NOT NULL PRIMARY KEY,
    store_id          UUID        NOT NULL,
    order_id          UUID        NOT NULL,
    secret_digest     BYTEA       NOT NULL,
    expires_at        TIMESTAMPTZ NOT NULL,
    revoked_at        TIMESTAMPTZ,
    last_used_at      TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, order_id),
    UNIQUE (store_id, secret_digest),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_keys_digest_check CHECK (octet_length(secret_digest) = 32),
    CONSTRAINT order_tracking_keys_expiry_check CHECK (expires_at > created_at),
    CONSTRAINT order_tracking_keys_revocation_check CHECK (
        revoked_at IS NULL OR revoked_at >= created_at
    )
);

CREATE TABLE commerce.order_tracking_sessions (
    id                UUID        NOT NULL PRIMARY KEY,
    store_id          UUID        NOT NULL,
    tracking_key_id   UUID        NOT NULL,
    access_digest     BYTEA       NOT NULL,
    expires_at        TIMESTAMPTZ NOT NULL,
    last_used_at      TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, access_digest),
    FOREIGN KEY (store_id, tracking_key_id)
        REFERENCES commerce.order_tracking_keys(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_sessions_digest_check CHECK (octet_length(access_digest) = 32),
    CONSTRAINT order_tracking_sessions_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX order_tracking_sessions_expiry_idx
    ON commerce.order_tracking_sessions (expires_at, id);

CREATE TABLE commerce.order_addresses (
    store_id             UUID               NOT NULL,
    order_id             UUID               NOT NULL,
    kind                 commerce.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (store_id, order_id, kind),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_addresses_full_name_length_check CHECK (
        length(trim(full_name)) BETWEEN 1 AND 200
    ),
    CONSTRAINT order_addresses_company_length_check CHECK (
        company IS NULL OR length(trim(company)) BETWEEN 1 AND 200
    ),
    CONSTRAINT order_addresses_line1_length_check CHECK (
        length(trim(address_line1)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_addresses_line2_length_check CHECK (
        address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_addresses_locality_length_check CHECK (
        length(trim(locality)) BETWEEN 1 AND 100
    ),
    CONSTRAINT order_addresses_area_length_check CHECK (
        administrative_area IS NULL
        OR length(trim(administrative_area)) BETWEEN 1 AND 100
    ),
    CONSTRAINT order_addresses_postal_code_length_check CHECK (
        postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT order_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE commerce.order_tax_calculations (
    store_id            UUID    NOT NULL,
    order_id            UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, tax_rule_id)
        REFERENCES commerce.tax_rules(store_id, id),
    CONSTRAINT order_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT order_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE commerce.order_promotion_calculations (
    store_id                      UUID                      NOT NULL,
    order_id                      UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       commerce.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    commerce.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, promotion_id)
        REFERENCES commerce.promotions(store_id, id),
    CONSTRAINT order_promotion_handle_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT order_promotion_name_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT order_promotion_trigger_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT order_promotion_value_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT order_promotion_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT order_promotion_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT order_promotion_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    ),
    CONSTRAINT order_promotion_discount_check CHECK (discount_amount_minor > 0),
    CONSTRAINT order_promotion_currency_check CHECK (currency ~ '^[A-Z]{3}$')
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
    discount_amount_minor    BIGINT      NOT NULL,
    tax_amount_minor         BIGINT      NOT NULL,
    total_amount_minor       BIGINT      NOT NULL,
    tax_inclusive            BOOLEAN     NOT NULL,
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
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
        AND (NOT tax_inclusive OR tax_amount_minor <= subtotal_amount_minor - discount_amount_minor)
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

CREATE TABLE commerce.order_fulfillment_transitions (
    id                       UUID                           NOT NULL PRIMARY KEY,
    store_id                 UUID                           NOT NULL,
    order_id                 UUID                           NOT NULL,
    source_event_id          UUID                           NOT NULL UNIQUE,
    from_fulfillment_status  commerce.order_fulfillment_status NOT NULL,
    to_fulfillment_status    commerce.order_fulfillment_status NOT NULL,
    from_delivery_status     commerce.order_delivery_status    NOT NULL,
    to_delivery_status       commerce.order_delivery_status    NOT NULL,
    occurred_at              TIMESTAMPTZ                    NOT NULL,

    UNIQUE (store_id, order_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id)
);

CREATE TABLE commerce.shipping_services (
    id                         UUID                                NOT NULL PRIMARY KEY,
    store_id                   UUID                                NOT NULL,
    code                       TEXT                                NOT NULL,
    name                       TEXT                                NOT NULL,
    amount_minor               BIGINT                              NOT NULL,
    currency                   CHAR(3)                             NOT NULL,
    estimated_min_days         SMALLINT                            NOT NULL,
    estimated_max_days         SMALLINT                            NOT NULL,
    status                     commerce.shipping_service_status NOT NULL DEFAULT 'active',
    created_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (store_id, currency)
        REFERENCES commerce.store_currencies(store_id, currency),
    CONSTRAINT shipping_services_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT shipping_services_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT shipping_services_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT shipping_services_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT shipping_services_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE commerce.shipping_provider_accounts (
    id                                   UUID        NOT NULL PRIMARY KEY,
    store_id                             UUID        NOT NULL,
    provider                             TEXT        NOT NULL,
    display_name                         TEXT        NOT NULL,
    credential_secret_reference          TEXT        NOT NULL,
    previous_credential_secret_reference TEXT,
    credential_rotation_expires_at       TIMESTAMPTZ,
    origin_name                          TEXT        NOT NULL,
    origin_company                       TEXT,
    origin_address_line_1                TEXT        NOT NULL,
    origin_address_line_2                TEXT,
    origin_city                          TEXT        NOT NULL,
    origin_region                        TEXT,
    origin_postal_code                   TEXT        NOT NULL,
    origin_country_code                  CHAR(2)     NOT NULL,
    origin_phone                         TEXT,
    origin_email                         TEXT,
    enabled                              BOOLEAN     NOT NULL DEFAULT false,
    created_by_user_id                   UUID,
    created_at                           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    CONSTRAINT shipping_provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT shipping_provider_accounts_provider_format_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT shipping_provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_credential_reference_check CHECK (
        credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT shipping_provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT shipping_provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT shipping_provider_accounts_origin_name_length_check CHECK (
        length(trim(origin_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_company_length_check CHECK (
        origin_company IS NULL OR length(trim(origin_company)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_address_length_check CHECK (
        length(trim(origin_address_line_1)) BETWEEN 1 AND 200
        AND (origin_address_line_2 IS NULL OR length(trim(origin_address_line_2)) BETWEEN 1 AND 200)
    ),
    CONSTRAINT shipping_provider_accounts_origin_city_length_check CHECK (
        length(trim(origin_city)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_region_length_check CHECK (
        origin_region IS NULL OR length(trim(origin_region)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_postal_length_check CHECK (
        length(trim(origin_postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT shipping_provider_accounts_origin_country_check CHECK (
        origin_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT shipping_provider_accounts_origin_phone_length_check CHECK (
        origin_phone IS NULL OR length(trim(origin_phone)) BETWEEN 1 AND 32
    ),
    CONSTRAINT shipping_provider_accounts_origin_email_length_check CHECK (
        origin_email IS NULL OR length(trim(origin_email)) BETWEEN 3 AND 254
    )
);

CREATE TABLE commerce.shipping_service_regions (
    store_id            UUID    NOT NULL,
    shipping_service_id UUID    NOT NULL,
    country_code        CHAR(2) NOT NULL,

    PRIMARY KEY (store_id, shipping_service_id, country_code),
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES commerce.shipping_services(store_id, id),
    CONSTRAINT shipping_service_regions_country_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE commerce.checkout_shipping_selections (
    store_id            UUID        NOT NULL,
    checkout_id         UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES commerce.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES commerce.shipping_services(store_id, id),
    CONSTRAINT checkout_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT checkout_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT checkout_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE commerce.order_shipping_selections (
    store_id            UUID        NOT NULL,
    order_id            UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES commerce.shipping_services(store_id, id),
    CONSTRAINT order_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT order_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT order_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE commerce.provider_accounts (
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    provider                   TEXT        NOT NULL,
    display_name               TEXT        NOT NULL DEFAULT 'Payment provider',
    external_account_reference TEXT        NOT NULL,
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
    CONSTRAINT provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    UNIQUE (provider, external_account_reference),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT provider_accounts_provider_length_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT provider_accounts_external_reference_length_check CHECK (
        length(trim(external_account_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT provider_accounts_credential_reference_check CHECK (
        credential_secret_reference IS NULL
        OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_webhook_reference_check CHECK (
        webhook_secret_reference IS NULL
        OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(webhook_secret_reference) <= 32768
            AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_previous_webhook_reference_check CHECK (
        previous_webhook_secret_reference IS NULL
        OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_webhook_secret_reference) <= 32768
            AND previous_webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_webhook_rotation_shape_check CHECK (
        (previous_webhook_secret_reference IS NULL AND webhook_rotation_expires_at IS NULL)
        OR (previous_webhook_secret_reference IS NOT NULL AND webhook_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_readiness_status_check CHECK (
        readiness_status IN ('unchecked', 'ready', 'action_required')
    ),
    CONSTRAINT provider_accounts_readiness_shape_check CHECK (
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
    CONSTRAINT provider_accounts_enabled_readiness_check CHECK (
        NOT enabled
        OR (
            readiness_status = 'ready'
            AND readiness_valid_until IS NOT NULL
            AND readiness_reconcile_at IS NOT NULL
        )
    ),
    CONSTRAINT provider_accounts_readiness_validity_check CHECK (
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
    CONSTRAINT provider_accounts_readiness_lock_shape_check CHECK (
        (readiness_locked_by IS NULL AND readiness_locked_at IS NULL)
        OR (readiness_locked_by IS NOT NULL AND readiness_locked_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_readiness_attempts_check CHECK (
        readiness_reconcile_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT provider_accounts_readiness_error_length_check CHECK (
        readiness_last_error IS NULL OR length(readiness_last_error) BETWEEN 1 AND 2000
    )
);

CREATE TABLE commerce.payment_attempts (
    id                     UUID                            NOT NULL PRIMARY KEY,
    store_id               UUID                            NOT NULL,
    order_id               UUID                            NOT NULL,
    shopper_id             UUID                            NOT NULL,
    provider_account_id    UUID                            NOT NULL,
    amount_minor           BIGINT                          NOT NULL,
    currency               CHAR(3)                         NOT NULL,
    status                 commerce.payment_attempt_status NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, id, currency),
    UNIQUE (provider_account_id, provider_reference),
    FOREIGN KEY (store_id, order_id, shopper_id)
        REFERENCES commerce.orders(store_id, id, shopper_id),
    FOREIGN KEY (store_id, order_id, currency)
        REFERENCES commerce.orders(store_id, id, currency),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.provider_accounts(store_id, id),
    CONSTRAINT payment_attempts_amount_positive_check CHECK (amount_minor > 0),
    CONSTRAINT payment_attempts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT payment_attempts_provider_reference_length_check CHECK (
        provider_reference IS NULL
        OR length(trim(provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT payment_attempts_failure_shape_check CHECK (
        (status = 'failed' AND failure_code IS NOT NULL)
        OR (status <> 'failed' AND failure_code IS NULL)
    )
);

CREATE TABLE commerce.refunds (
    id                     UUID                    NOT NULL PRIMARY KEY,
    store_id               UUID                    NOT NULL,
    payment_attempt_id     UUID                    NOT NULL,
    amount_minor           BIGINT                  NOT NULL,
    currency               CHAR(3)                 NOT NULL,
    status                 commerce.refund_status  NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (payment_attempt_id, provider_reference),
    FOREIGN KEY (store_id, payment_attempt_id, currency)
        REFERENCES commerce.payment_attempts(store_id, id, currency),
    CONSTRAINT refunds_amount_positive_check CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_provider_reference_length_check CHECK (
        provider_reference IS NULL
        OR length(trim(provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT refunds_failure_shape_check CHECK (
        (status = 'failed' AND failure_code IS NOT NULL)
        OR (status <> 'failed' AND failure_code IS NULL)
    )
);

CREATE TABLE commerce.fulfillments (
    id                   UUID                           NOT NULL PRIMARY KEY,
    store_id             UUID                           NOT NULL,
    order_id             UUID                           NOT NULL,
    status               commerce.fulfillment_status NOT NULL DEFAULT 'pending',
    carrier              TEXT,
    tracking_number      TEXT,
    shipped_at           TIMESTAMPTZ,
    delivered_at         TIMESTAMPTZ,
    cancelled_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, carrier, tracking_number),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    CONSTRAINT fulfillments_tracking_shape_check CHECK (
        (status = 'pending' AND carrier IS NULL AND tracking_number IS NULL
            AND shipped_at IS NULL AND delivered_at IS NULL AND cancelled_at IS NULL)
        OR (status = 'shipped' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND shipped_at IS NOT NULL AND delivered_at IS NULL AND cancelled_at IS NULL)
        OR (status = 'delivered' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND shipped_at IS NOT NULL AND delivered_at IS NOT NULL AND cancelled_at IS NULL)
        OR (status = 'cancelled' AND shipped_at IS NULL AND delivered_at IS NULL
            AND cancelled_at IS NOT NULL)
    ),
    CONSTRAINT fulfillments_carrier_length_check CHECK (
        carrier IS NULL OR length(trim(carrier)) BETWEEN 1 AND 64
    ),
    CONSTRAINT fulfillments_tracking_number_length_check CHECK (
        tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255
    )
);

CREATE TABLE commerce.fulfillment_lines (
    store_id             UUID    NOT NULL,
    fulfillment_id       UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             INTEGER NOT NULL,

    PRIMARY KEY (store_id, fulfillment_id, product_variant_id),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES commerce.fulfillments(store_id, id),
    CONSTRAINT fulfillment_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999)
);

CREATE TABLE commerce.shipping_quote_requests (
    id                    UUID        NOT NULL PRIMARY KEY,
    store_id              UUID        NOT NULL,
    fulfillment_id        UUID        NOT NULL,
    provider_account_id   UUID        NOT NULL,
    idempotency_key       TEXT        NOT NULL,
    request_fingerprint   BYTEA       NOT NULL,
    length_millimetres    INTEGER     NOT NULL,
    width_millimetres     INTEGER     NOT NULL,
    height_millimetres    INTEGER     NOT NULL,
    weight_grams          INTEGER     NOT NULL,
    state                 TEXT        NOT NULL DEFAULT 'pending',
    expires_at            TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, idempotency_key),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES commerce.fulfillments(store_id, id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.shipping_provider_accounts(store_id, id),
    CONSTRAINT shipping_quote_requests_idempotency_length_check CHECK (
        length(idempotency_key) BETWEEN 1 AND 128
    ),
    CONSTRAINT shipping_quote_requests_fingerprint_length_check CHECK (
        octet_length(request_fingerprint) = 32
    ),
    CONSTRAINT shipping_quote_requests_parcel_check CHECK (
        length_millimetres BETWEEN 1 AND 10000
        AND width_millimetres BETWEEN 1 AND 10000
        AND height_millimetres BETWEEN 1 AND 10000
        AND weight_grams BETWEEN 1 AND 1000000
    ),
    CONSTRAINT shipping_quote_requests_state_check CHECK (state IN ('pending', 'completed')),
    CONSTRAINT shipping_quote_requests_completion_check CHECK (
        (state = 'pending' AND expires_at IS NULL)
        OR (state = 'completed' AND expires_at IS NOT NULL)
    )
);

CREATE TABLE commerce.shipping_rate_quotes (
    id                            UUID        NOT NULL PRIMARY KEY,
    store_id                      UUID        NOT NULL,
    quote_request_id              UUID        NOT NULL,
    provider_shipment_reference   TEXT        NOT NULL,
    provider_rate_reference       TEXT        NOT NULL,
    carrier                       TEXT        NOT NULL,
    service                       TEXT        NOT NULL,
    amount_minor                  BIGINT      NOT NULL,
    currency                      CHAR(3)     NOT NULL,
    estimated_delivery_days       SMALLINT,
    guaranteed                    BOOLEAN     NOT NULL,
    expires_at                    TIMESTAMPTZ NOT NULL,
    created_at                    TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, quote_request_id, provider_rate_reference),
    FOREIGN KEY (store_id, quote_request_id)
        REFERENCES commerce.shipping_quote_requests(store_id, id),
    CONSTRAINT shipping_rate_quotes_shipment_reference_length_check CHECK (
        length(trim(provider_shipment_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_rate_quotes_rate_reference_length_check CHECK (
        length(trim(provider_rate_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_rate_quotes_carrier_length_check CHECK (
        length(trim(carrier)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_rate_quotes_service_length_check CHECK (
        length(trim(service)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_rate_quotes_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT shipping_rate_quotes_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT shipping_rate_quotes_delivery_days_check CHECK (
        estimated_delivery_days IS NULL OR estimated_delivery_days BETWEEN 0 AND 365
    )
);

CREATE TABLE commerce.shipping_labels (
    id                              UUID        NOT NULL PRIMARY KEY,
    store_id                        UUID        NOT NULL,
    fulfillment_id                  UUID        NOT NULL,
    provider_account_id             UUID        NOT NULL,
    rate_quote_id                   UUID        NOT NULL,
    purchase_idempotency_key        TEXT        NOT NULL,
    purchase_request_fingerprint    BYTEA       NOT NULL,
    purchase_state                  TEXT        NOT NULL DEFAULT 'purchasing',
    provider_shipment_reference     TEXT        NOT NULL,
    provider_rate_reference         TEXT        NOT NULL,
    carrier                         TEXT,
    tracking_number                 TEXT,
    provider_tracker_reference      TEXT,
    label_url                       TEXT,
    label_media_type                TEXT,
    cancellation_idempotency_key    TEXT,
    cancellation_request_fingerprint BYTEA,
    cancellation_status             TEXT,
    cancellation_reconcile_at       TIMESTAMPTZ,
    cancellation_locked_by          UUID,
    cancellation_locked_at          TIMESTAMPTZ,
    cancellation_attempts           INTEGER     NOT NULL DEFAULT 0,
    cancellation_last_error         TEXT,
    tracking_status                 TEXT,
    tracking_status_detail          TEXT,
    estimated_delivery_at           TIMESTAMPTZ,
    tracking_observed_at            TIMESTAMPTZ,
    next_tracking_refresh_at        TIMESTAMPTZ,
    tracking_locked_by              UUID,
    tracking_locked_at              TIMESTAMPTZ,
    tracking_attempts               INTEGER     NOT NULL DEFAULT 0,
    tracking_last_error             TEXT,
    purchased_at                    TIMESTAMPTZ,
    cancellation_requested_at       TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, fulfillment_id),
    CONSTRAINT shipping_labels_purchase_idempotency_key
        UNIQUE (store_id, purchase_idempotency_key),
    CONSTRAINT shipping_labels_cancellation_idempotency_key
        UNIQUE (store_id, cancellation_idempotency_key),
    UNIQUE (carrier, tracking_number),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES commerce.fulfillments(store_id, id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.shipping_provider_accounts(store_id, id),
    FOREIGN KEY (store_id, rate_quote_id)
        REFERENCES commerce.shipping_rate_quotes(store_id, id),
    CONSTRAINT shipping_labels_purchase_idempotency_length_check CHECK (
        length(purchase_idempotency_key) BETWEEN 1 AND 128
    ),
    CONSTRAINT shipping_labels_purchase_fingerprint_length_check CHECK (
        octet_length(purchase_request_fingerprint) = 32
    ),
    CONSTRAINT shipping_labels_purchase_state_check CHECK (
        purchase_state IN ('purchasing', 'purchased')
    ),
    CONSTRAINT shipping_labels_provider_reference_length_check CHECK (
        length(trim(provider_shipment_reference)) BETWEEN 1 AND 255
        AND length(trim(provider_rate_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_purchase_shape_check CHECK (
        (purchase_state = 'purchasing' AND carrier IS NULL AND tracking_number IS NULL
            AND label_url IS NULL AND label_media_type IS NULL AND purchased_at IS NULL)
        OR (purchase_state = 'purchased' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND label_url IS NOT NULL AND label_media_type IS NOT NULL AND purchased_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_carrier_length_check CHECK (
        carrier IS NULL OR length(trim(carrier)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_labels_tracking_number_length_check CHECK (
        tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_tracker_reference_length_check CHECK (
        provider_tracker_reference IS NULL
        OR length(trim(provider_tracker_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_label_url_check CHECK (
        label_url IS NULL OR (length(label_url) BETWEEN 9 AND 2048 AND label_url ~ '^https://')
    ),
    CONSTRAINT shipping_labels_media_type_length_check CHECK (
        label_media_type IS NULL OR length(trim(label_media_type)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_labels_cancellation_shape_check CHECK (
        (cancellation_idempotency_key IS NULL AND cancellation_request_fingerprint IS NULL
            AND cancellation_status IS NULL AND cancellation_requested_at IS NULL)
        OR (length(cancellation_idempotency_key) BETWEEN 1 AND 128
            AND octet_length(cancellation_request_fingerprint) = 32
            AND cancellation_status IN ('submitted', 'cancelled', 'rejected', 'not_available')
            AND cancellation_requested_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_cancellation_lock_shape_check CHECK (
        (cancellation_locked_by IS NULL AND cancellation_locked_at IS NULL)
        OR (cancellation_locked_by IS NOT NULL AND cancellation_locked_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_cancellation_attempts_check CHECK (
        cancellation_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT shipping_labels_cancellation_error_length_check CHECK (
        cancellation_last_error IS NULL OR length(cancellation_last_error) BETWEEN 1 AND 2000
    ),
    CONSTRAINT shipping_labels_tracking_status_check CHECK (
        tracking_status IS NULL
        OR tracking_status IN ('pre_transit', 'in_transit', 'out_for_delivery', 'delivered', 'failure', 'unknown')
    ),
    CONSTRAINT shipping_labels_tracking_detail_length_check CHECK (
        tracking_status_detail IS NULL OR length(tracking_status_detail) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_tracking_observation_shape_check CHECK (
        (tracking_status IS NULL AND tracking_observed_at IS NULL)
        OR (tracking_status IS NOT NULL AND tracking_observed_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_tracking_lock_shape_check CHECK (
        (tracking_locked_by IS NULL AND tracking_locked_at IS NULL)
        OR (tracking_locked_by IS NOT NULL AND tracking_locked_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_tracking_attempts_check CHECK (
        tracking_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT shipping_labels_tracking_error_length_check CHECK (
        tracking_last_error IS NULL OR length(tracking_last_error) BETWEEN 1 AND 2000
    )
);

CREATE TABLE commerce.returns (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    order_id             UUID                      NOT NULL,
    status               commerce.return_status NOT NULL DEFAULT 'requested',
    refund_id            UUID,
    refund_amount_minor  BIGINT                    NOT NULL,
    currency             CHAR(3)                   NOT NULL,
    requested_at         TIMESTAMPTZ               NOT NULL,
    authorized_at        TIMESTAMPTZ,
    received_at          TIMESTAMPTZ,
    completed_at         TIMESTAMPTZ,
    rejected_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    FOREIGN KEY (store_id, order_id, currency)
        REFERENCES commerce.orders(store_id, id, currency),
    FOREIGN KEY (store_id, refund_id)
        REFERENCES commerce.refunds(store_id, id),
    CONSTRAINT returns_status_timestamps_check CHECK (
        (status = 'requested' AND authorized_at IS NULL AND received_at IS NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'authorized' AND authorized_at IS NOT NULL AND received_at IS NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'received' AND authorized_at IS NOT NULL AND received_at IS NOT NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'completed' AND authorized_at IS NOT NULL AND received_at IS NOT NULL
            AND completed_at IS NOT NULL AND rejected_at IS NULL)
        OR (status = 'rejected' AND received_at IS NULL AND completed_at IS NULL
            AND rejected_at IS NOT NULL)
    ),
    CONSTRAINT returns_refund_amount_nonnegative_check CHECK (refund_amount_minor >= 0),
    CONSTRAINT returns_currency_format_check CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE commerce.return_lines (
    store_id             UUID                           NOT NULL,
    return_id            UUID                           NOT NULL,
    product_variant_id   UUID                           NOT NULL,
    inventory_location_id UUID,
    quantity             INTEGER                        NOT NULL,
    refund_amount_minor  BIGINT                         NOT NULL,
    disposition          commerce.return_disposition,

    PRIMARY KEY (store_id, return_id, product_variant_id),
    FOREIGN KEY (store_id, return_id)
        REFERENCES commerce.returns(store_id, id),
    FOREIGN KEY (store_id, inventory_location_id)
        REFERENCES commerce.inventory_locations(store_id, id),
    CONSTRAINT return_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT return_lines_refund_amount_nonnegative_check CHECK (refund_amount_minor >= 0),
    CONSTRAINT return_lines_restock_location_check CHECK (
        disposition <> 'restock' OR inventory_location_id IS NOT NULL
    )
);

CREATE INDEX customers_store_created_idx
    ON commerce.customers (store_id, created_at DESC, id DESC);

CREATE INDEX customer_addresses_customer_idx
    ON commerce.customer_addresses (store_id, customer_id, created_at, id);

CREATE INDEX customer_shopper_links_history_idx
    ON commerce.customer_shopper_links (store_id, customer_id, sales_channel_id, shopper_id
    );

CREATE INDEX carts_channel_updated_idx
    ON commerce.carts (store_id,
        sales_channel_id,
        status,
        updated_at DESC,
        id DESC
    );

CREATE INDEX cart_lines_variant_lookup_idx
    ON commerce.cart_lines (store_id, product_variant_id, cart_id);

CREATE INDEX checkouts_channel_created_idx
    ON commerce.checkouts (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX checkouts_expiry_claim_idx
    ON commerce.checkouts (expires_at, id)
    WHERE status = 'pending';

CREATE INDEX orders_channel_created_idx
    ON commerce.orders (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX orders_customer_created_idx
    ON commerce.orders (store_id, customer_id, created_at DESC, id DESC
    ) WHERE customer_id IS NOT NULL;

CREATE INDEX order_transitions_order_time_idx
    ON commerce.order_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

CREATE INDEX order_fulfillment_transitions_order_idx
    ON commerce.order_fulfillment_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

CREATE INDEX provider_accounts_store_created_idx
    ON commerce.provider_accounts (store_id, created_at DESC, id DESC);

CREATE INDEX provider_accounts_readiness_due_idx
    ON commerce.provider_accounts (readiness_reconcile_at, id)
    WHERE enabled;

CREATE INDEX payment_attempts_order_created_idx
    ON commerce.payment_attempts (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX refunds_attempt_created_idx
    ON commerce.refunds (store_id,
        payment_attempt_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX shipping_services_quote_idx
    ON commerce.shipping_services (store_id,
        currency,
        status,
        id
    );

CREATE INDEX shipping_provider_accounts_store_created_idx
    ON commerce.shipping_provider_accounts (store_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX shipping_service_regions_quote_idx
    ON commerce.shipping_service_regions (store_id,
        country_code,
        shipping_service_id
    );

CREATE INDEX fulfillments_order_created_idx
    ON commerce.fulfillments (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX fulfillment_lines_variant_idx
    ON commerce.fulfillment_lines (store_id,
        product_variant_id,
        fulfillment_id
    );

CREATE INDEX shipping_quote_requests_fulfillment_created_idx
    ON commerce.shipping_quote_requests (store_id,
        fulfillment_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX shipping_rate_quotes_request_expiry_idx
    ON commerce.shipping_rate_quotes (store_id,
        quote_request_id,
        expires_at,
        id
    );

CREATE INDEX shipping_labels_tracking_due_idx
    ON commerce.shipping_labels (next_tracking_refresh_at, id)
    WHERE purchase_state = 'purchased' AND next_tracking_refresh_at IS NOT NULL;

CREATE INDEX shipping_labels_cancellation_due_idx
    ON commerce.shipping_labels (cancellation_reconcile_at, id)
    WHERE cancellation_status = 'submitted' AND cancellation_reconcile_at IS NOT NULL;

CREATE INDEX returns_order_created_idx
    ON commerce.returns (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX return_lines_variant_idx
    ON commerce.return_lines (store_id,
        product_variant_id,
        return_id
    );

CREATE FUNCTION commerce.claim_expired_checkouts(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    inventory_reservation_id UUID
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT checkout.id
        FROM commerce.checkouts AS checkout
        WHERE checkout.status = 'pending'
          AND checkout.expires_at <= claimed_at
          AND (
              checkout.expiry_locked_at IS NULL
              OR checkout.expiry_locked_at <= stale_before
          )
        ORDER BY checkout.expires_at, checkout.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 500), 1)
    )
    UPDATE commerce.checkouts AS checkout
       SET expiry_locked_by = worker_id,
           expiry_locked_at = claimed_at
      FROM claimable
     WHERE checkout.id = claimable.id
    RETURNING checkout.id, checkout.store_id,
              checkout.inventory_reservation_id;
$$;

CREATE FUNCTION commerce.provider_readiness_metrics()
RETURNS TABLE (
    due BIGINT,
    retrying BIGINT,
    expiring_within_six_hours BIGINT,
    action_required BIGINT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_reconcile_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_reconcile_attempts > 0
           ),
           count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_valid_until <= CURRENT_TIMESTAMP + INTERVAL '6 hours'
           ),
           count(*) FILTER (WHERE account.readiness_status = 'action_required')
      FROM commerce.provider_accounts AS account;
$$;

CREATE FUNCTION commerce.resolve_provider_account(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
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
    FROM commerce.provider_accounts AS account
    WHERE account.provider = requested_provider
      AND account.external_account_reference = requested_external_account_reference;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
)
RETURNS TABLE (
    external_account_reference TEXT,
    secret_reference           TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.external_account_reference, candidate.secret_reference
    FROM commerce.provider_accounts AS account
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
      AND (
          requested_external_account_reference IS NULL
          OR account.external_account_reference = requested_external_account_reference
    )
      AND account.enabled
      AND candidate.secret_reference IS NOT NULL
    ORDER BY account.id, candidate.priority;
$$;

CREATE FUNCTION commerce.claim_provider_readiness_checks(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id UUID,
    provider TEXT,
    external_account_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE commerce.provider_accounts AS account
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
        FROM commerce.provider_accounts AS account
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
    UPDATE commerce.provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.external_account_reference, account.credential_secret_reference,
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
    UPDATE commerce.provider_accounts AS account
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

CREATE FUNCTION commerce.shipping_tracking_metrics()
RETURNS TABLE (
    due BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_due_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE label.next_tracking_refresh_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (WHERE label.tracking_locked_by IS NOT NULL),
           count(*) FILTER (
               WHERE label.next_tracking_refresh_at IS NULL
                 AND label.tracking_attempts >= 8
                 AND label.tracking_last_error IS NOT NULL
           ),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(label.next_tracking_refresh_at)
                            FILTER (WHERE label.next_tracking_refresh_at <= CURRENT_TIMESTAMP))
               ),
               0
           )::DOUBLE PRECISION
      FROM commerce.shipping_labels AS label
     WHERE label.purchase_state = 'purchased'
       AND label.provider_tracker_reference IS NOT NULL;
$$;

CREATE FUNCTION commerce.shipping_cancellation_metrics()
RETURNS TABLE (
    due BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_due_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE label.cancellation_reconcile_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (WHERE label.cancellation_locked_by IS NOT NULL),
           count(*) FILTER (
               WHERE label.cancellation_reconcile_at IS NULL
                 AND label.cancellation_attempts >= 8
                 AND label.cancellation_last_error IS NOT NULL
           ),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(label.cancellation_reconcile_at)
                            FILTER (WHERE label.cancellation_reconcile_at <= CURRENT_TIMESTAMP))
               ),
               0
           )::DOUBLE PRECISION
      FROM commerce.shipping_labels AS label
     WHERE label.cancellation_status = 'submitted';
$$;

CREATE FUNCTION commerce.claim_shipping_tracking(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    store_id UUID,
    fulfillment_id UUID,
    provider TEXT,
    provider_tracker_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT label.id
        FROM commerce.shipping_labels AS label
        WHERE label.purchase_state = 'purchased'
          AND label.provider_tracker_reference IS NOT NULL
          AND label.next_tracking_refresh_at <= claimed_at
          AND (
              label.tracking_locked_at IS NULL
              OR label.tracking_locked_at <= stale_before
          )
        ORDER BY label.next_tracking_refresh_at, label.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE commerce.shipping_labels AS label
       SET tracking_locked_by = worker_id,
           tracking_locked_at = claimed_at,
           tracking_attempts = least(label.tracking_attempts, 30) + 1
      FROM claimable,
           commerce.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_tracker_reference,
              account.credential_secret_reference, label.tracking_attempts;
$$;

CREATE FUNCTION commerce.claim_shipping_cancellations(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    store_id UUID,
    fulfillment_id UUID,
    provider TEXT,
    provider_shipment_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT label.id
        FROM commerce.shipping_labels AS label
        WHERE label.cancellation_status = 'submitted'
          AND label.cancellation_reconcile_at <= claimed_at
          AND (
              label.cancellation_locked_at IS NULL
              OR label.cancellation_locked_at <= stale_before
          )
        ORDER BY label.cancellation_reconcile_at, label.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE commerce.shipping_labels AS label
       SET cancellation_locked_by = worker_id,
           cancellation_locked_at = claimed_at,
           cancellation_attempts = least(label.cancellation_attempts, 30) + 1
      FROM claimable,
           commerce.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_shipment_reference,
              account.credential_secret_reference, label.cancellation_attempts;
$$;

ALTER TABLE commerce.customers ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.customer_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.customer_shopper_links ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkouts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_contacts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_tax_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_promotion_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_tracking_keys ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_tracking_sessions ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_contacts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_tax_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_promotion_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_transitions ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_fulfillment_transitions ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.checkout_shipping_selections ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_shipping_selections ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.provider_accounts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.payment_attempts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.fulfillments ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_services ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_provider_accounts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_service_regions ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.fulfillment_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_quote_requests ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_rate_quotes ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.shipping_labels ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.returns ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.return_lines ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.carts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.customers
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.customer_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.customer_shopper_links
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

CREATE POLICY store_isolation ON commerce.checkouts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_contacts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_tax_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_promotion_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_lines
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

CREATE POLICY store_isolation ON commerce.order_tracking_keys
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_tracking_sessions
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_contacts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_tax_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_promotion_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

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

CREATE POLICY store_isolation ON commerce.order_fulfillment_transitions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.checkout_shipping_selections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_shipping_selections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.provider_accounts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.payment_attempts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.refunds
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.fulfillments
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_services
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_provider_accounts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_service_regions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.fulfillment_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_quote_requests
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_rate_quotes
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.shipping_labels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.returns
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.return_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

COMMENT ON INDEX commerce.checkouts_expiry_claim_idx IS
    'Supports the cross-tenant SECURITY DEFINER expiry scheduler claim path';

REVOKE ALL ON FUNCTION commerce.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON commerce.customer_shopper_links FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON commerce.checkout_contacts, commerce.checkout_addresses, commerce.checkout_lines,
       commerce.checkout_tax_calculations,
       commerce.checkout_promotion_calculations FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON commerce.order_contacts, commerce.order_addresses, commerce.order_lines,
       commerce.order_tax_calculations,
       commerce.order_promotion_calculations, commerce.order_transitions,
       commerce.order_fulfillment_transitions
    FROM chaos_runtime;

REVOKE DELETE ON commerce.checkouts, commerce.orders FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON commerce.checkout_shipping_selections FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON commerce.order_shipping_selections FROM chaos_runtime;

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(TEXT, TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.provider_readiness_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, TEXT) TO chaos_runtime;

GRANT EXECUTE
    ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, TEXT) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.provider_readiness_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE ALL ON FUNCTION commerce.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.shipping_tracking_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.shipping_cancellation_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.shipping_tracking_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.shipping_cancellation_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
