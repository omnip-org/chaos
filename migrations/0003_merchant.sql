CREATE TYPE merchant.store_role AS ENUM ('owner', 'member');

CREATE TYPE merchant.store_status AS ENUM ('active', 'inactive');

CREATE TYPE merchant.sales_channel_kind AS ENUM (
    'web',
    'mobile',
    'point_of_sale',
    'marketplace',
    'custom'
);

CREATE TYPE merchant.sales_channel_status AS ENUM ('active', 'archived');

CREATE TYPE merchant.store_locale_event_kind AS ENUM ('enabled', 'disabled', 'default_changed');

CREATE TYPE merchant.api_key_class AS ENUM ('publishable', 'secret');

CREATE TYPE merchant.api_key_scope AS ENUM (
    'analytics:write',
    'catalog:read',
    'carts:write',
    'checkout:write',
    'orders:read',
    'customers:write',
    'mcp:tools',
    'products:read',
    'products:write',
    'pricing:read',
    'pricing:write',
    'inventory:read',
    'inventory:write',
    'collections:read',
    'collections:write',
    'orders:write',
    'fulfillment:read',
    'fulfillment:write',
    'store_admin:read',
    'store_admin:write',
    'payments:write',
    'media:read',
    'media:write',
    'reviews:write',
    'api_keys:read',
    'api_keys:write',
    'provider_secrets:write'
);

CREATE TABLE merchant.stores (
    id                   UUID                     NOT NULL PRIMARY KEY,
    code                 extensions.citext        NOT NULL UNIQUE,
    name                 TEXT                     NOT NULL,
    default_region       CHAR(2)                  NOT NULL DEFAULT 'US',
    default_currency     CHAR(3)                  NOT NULL DEFAULT 'USD',
    default_locale       VARCHAR(63)              NOT NULL DEFAULT 'en-US',
    status               merchant.store_status    NOT NULL DEFAULT 'inactive',
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

CREATE TABLE merchant.store_memberships (
    store_id    UUID                    NOT NULL,
    user_id     UUID                    NOT NULL,
    role        merchant.store_role     NOT NULL,
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, user_id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE
);

CREATE TABLE merchant.store_locales (
    store_id            UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    created_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, locale),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT store_locales_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE merchant.store_locale_events (
    id                  UUID                             NOT NULL PRIMARY KEY,
    store_id            UUID                             NOT NULL,
    locale              VARCHAR(63)                      NOT NULL,
    previous_locale     VARCHAR(63),
    event_kind          merchant.store_locale_event_kind NOT NULL,
    actor_user_id       UUID                             NOT NULL,
    occurred_at         TIMESTAMPTZ                      NOT NULL,

    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
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

CREATE TABLE merchant.sales_channels (
    id                   UUID                              NOT NULL PRIMARY KEY,
    store_id             UUID                              NOT NULL,
    code                 extensions.citext                 NOT NULL,
    name                 TEXT                              NOT NULL,
    kind                 merchant.sales_channel_kind       NOT NULL,
    status               merchant.sales_channel_status     NOT NULL DEFAULT 'active',
    is_default           BOOLEAN                           NOT NULL DEFAULT false,
    created_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT sales_channels_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT sales_channels_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE merchant.store_currencies (
    store_id             UUID       NOT NULL,
    currency             CHAR(3)    NOT NULL,
    enabled              BOOLEAN    NOT NULL DEFAULT true,

    PRIMARY KEY (store_id, currency),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT store_currencies_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    )
);

CREATE TABLE merchant.api_keys (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    sales_channel_id     UUID,
    key_identifier       TEXT                      NOT NULL UNIQUE,
    secret_digest        BYTEA                     NOT NULL,
    display_suffix       CHAR(4)                   NOT NULL,
    name                 TEXT                      NOT NULL,
    class                merchant.api_key_class    NOT NULL,
    created_by_user_id   UUID                      NOT NULL,
    revoked_by_user_id   UUID,
    expires_at           TIMESTAMPTZ,
    last_used_at         TIMESTAMPTZ,
    revoked_at           TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    FOREIGN KEY (created_by_user_id)
        REFERENCES identity.users(id),
    FOREIGN KEY (revoked_by_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT api_keys_identifier_format_check CHECK (
        key_identifier ~ '^[A-Za-z0-9_-]{16}$'
    ),
    CONSTRAINT api_keys_secret_digest_length_check CHECK (
        octet_length(secret_digest) = 32
    ),
    CONSTRAINT api_keys_display_suffix_format_check CHECK (
        display_suffix ~ '^[A-Za-z0-9_-]{4}$'
    ),
    CONSTRAINT api_keys_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    ),
    CONSTRAINT api_keys_expiration_check CHECK (
        expires_at IS NULL OR expires_at > created_at
    ),
    CONSTRAINT api_keys_revocation_check CHECK (
        (revoked_at IS NULL AND revoked_by_user_id IS NULL)
        OR (revoked_at IS NOT NULL AND revoked_by_user_id IS NOT NULL)
    )
);

CREATE TABLE merchant.api_key_scopes (
    api_key_id           UUID                      NOT NULL,
    scope                merchant.api_key_scope    NOT NULL,

    PRIMARY KEY (api_key_id, scope),
    FOREIGN KEY (api_key_id)
        REFERENCES merchant.api_keys(id) ON DELETE CASCADE
);

CREATE INDEX store_memberships_user_idx
    ON merchant.store_memberships (user_id, store_id);

CREATE INDEX stores_status_idx
    ON merchant.stores (status);

CREATE INDEX store_locale_events_store_occurred_idx
    ON merchant.store_locale_events (store_id, occurred_at, id);

CREATE UNIQUE INDEX sales_channels_one_default_per_store_idx
    ON merchant.sales_channels (store_id)
    WHERE is_default;

CREATE INDEX sales_channels_store_status_idx
    ON merchant.sales_channels (store_id, status);

CREATE INDEX api_keys_store_created_idx
    ON merchant.api_keys (store_id, created_at DESC, id DESC);

CREATE FUNCTION merchant.prevent_default_locale_removal()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM merchant.stores
        WHERE id = OLD.store_id
          AND default_locale = OLD.locale
    ) THEN
        RAISE EXCEPTION 'the default Store Locale cannot be disabled'
            USING ERRCODE = '23514';
    END IF;
    RETURN OLD;
END
$$;

CREATE FUNCTION merchant.authenticate_api_key(
    presented_key_identifier  TEXT,
    presented_secret_digest  BYTEA
)
RETURNS TABLE (
    api_key_id           UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    class                TEXT,
    scopes               TEXT[],
    created_by_user_id   UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT api_key.id,
           api_key.store_id,
           sales_channel.id,
           api_key.class::TEXT,
           ARRAY(
               SELECT api_key_scope.scope::TEXT
               FROM merchant.api_key_scopes AS api_key_scope
               WHERE api_key_scope.api_key_id = api_key.id
               ORDER BY api_key_scope.scope::TEXT
           ),
           api_key.created_by_user_id
    FROM merchant.api_keys AS api_key
    INNER JOIN merchant.stores AS store
        ON store.id = api_key.store_id
    LEFT JOIN merchant.sales_channels AS sales_channel
        ON sales_channel.store_id = api_key.store_id
       AND sales_channel.id = COALESCE(
           api_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM merchant.sales_channels AS default_channel
               WHERE default_channel.store_id = api_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE api_key.key_identifier = presented_key_identifier
      AND api_key.secret_digest = presented_secret_digest
      AND api_key.revoked_at IS NULL
      AND (api_key.expires_at IS NULL OR api_key.expires_at > CURRENT_TIMESTAMP)
      AND store.status = 'active';
$$;

CREATE TRIGGER store_locales_protect_default
BEFORE DELETE ON merchant.store_locales
FOR EACH ROW EXECUTE FUNCTION merchant.prevent_default_locale_removal();

ALTER TABLE merchant.stores ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_memberships ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_locales ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_locale_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_currencies ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.sales_channels ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.api_keys ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.api_key_scopes ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON merchant.stores
    USING (id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_directory ON merchant.stores
    FOR SELECT
    USING (
        EXISTS (
            SELECT 1
            FROM merchant.store_memberships AS membership
            WHERE membership.store_id = stores.id
              AND membership.user_id =
                    nullif(current_setting('app.user_id', true), '')::uuid
        )
    );

CREATE POLICY store_isolation ON merchant.store_memberships
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_membership_directory ON merchant.store_memberships
    FOR SELECT
    USING (
        user_id = nullif(current_setting('app.user_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_locales
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_locale_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_currencies
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.sales_channels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.api_keys
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.api_key_scopes
    USING (
        EXISTS (
            SELECT 1
            FROM merchant.api_keys AS api_key
            WHERE api_key.id = api_key_scopes.api_key_id
              AND api_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    )
    WITH CHECK (
        EXISTS (
            SELECT 1
            FROM merchant.api_keys AS api_key
            WHERE api_key.id = api_key_scopes.api_key_id
              AND api_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    );

REVOKE ALL ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) FROM PUBLIC;

COMMENT ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) IS
    'Authenticates a machine credential without exposing stored secret digests';

GRANT EXECUTE
    ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA merchant TO chaos_runtime;

REVOKE UPDATE, DELETE ON merchant.store_locale_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA merchant TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
