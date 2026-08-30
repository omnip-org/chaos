CREATE SCHEMA commerce;

CREATE TYPE commerce.store_role AS ENUM ('owner', 'member');
CREATE TYPE commerce.store_status AS ENUM ('active', 'inactive');
CREATE TYPE commerce.sales_channel_status AS ENUM ('active', 'archived');

CREATE TABLE commerce.stores (
    id          UUID                     NOT NULL PRIMARY KEY,
    name        TEXT                     NOT NULL,
    region      CHAR(2)                  NOT NULL DEFAULT 'US',
    currency    CHAR(3)                  NOT NULL DEFAULT 'USD',
    meta        JSONB,
    status      commerce.store_status    NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT stores_name_length_check           CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT stores_region_format_check         CHECK (region ~ '^[A-Z]{2}$'),
    CONSTRAINT stores_currency_format_check       CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT stores_meta_size_check             CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT stores_meta_is_object_check        CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object')
);

CREATE TABLE commerce.shoppers (
    id           UUID          NOT NULL PRIMARY KEY,
    store_id     UUID          NOT NULL,
    meta         JSONB,
    last_seen_at TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at   TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT shoppers_store_id_id_key       UNIQUE (store_id, id),
    CONSTRAINT shoppers_store_id_fkey         FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT shoppers_meta_size_check       CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT shoppers_meta_is_object_check  CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object')
);

CREATE TABLE commerce.store_memberships (
    store_id   UUID                 NOT NULL,
    user_id    UUID                 NOT NULL,
    role       commerce.store_role  NOT NULL,
    created_at TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT store_memberships_pkey               PRIMARY KEY (store_id, user_id),
    CONSTRAINT store_memberships_store_id_fkey      FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT store_memberships_user_id_fkey       FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE
);

CREATE TABLE commerce.channels (
    id                UUID                           NOT NULL PRIMARY KEY,
    store_id          UUID                           NOT NULL,
    name              TEXT                           NOT NULL,
    origin            TEXT                           NOT NULL,
    status            commerce.sales_channel_status  NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at        TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT channels_store_id_id_key        UNIQUE (store_id, id),
    CONSTRAINT channels_origin_key             UNIQUE (origin),
    CONSTRAINT channels_store_id_fkey          FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT channels_name_length_check      CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT channels_origin_length_check    CHECK (length(trim(origin)) BETWEEN 10 AND 2048),
    CONSTRAINT channels_origin_scheme_check    CHECK (origin ~ '^https?://')
);

CREATE TABLE commerce.channel_publishable_keys (
    id               UUID          NOT NULL PRIMARY KEY,
    store_id         UUID          NOT NULL,
    channel_id       UUID          NOT NULL,
    public_key       TEXT          NOT NULL UNIQUE,
    name             TEXT          NOT NULL,
    revoked_at       TIMESTAMPTZ,
    created_at       TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT channel_publishable_keys_store_id_fkey         FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT channel_publishable_keys_store_id_channel_fkey FOREIGN KEY (store_id, channel_id) REFERENCES commerce.channels (store_id, id),
    CONSTRAINT channel_publishable_keys_public_key_format     CHECK (public_key ~ '^pk_[1-9A-HJ-NP-Za-km-z]{24}$'),
    CONSTRAINT channel_publishable_keys_name_length           CHECK (length(trim(name)) BETWEEN 1 AND 80)
);

CREATE TABLE commerce.store_shipping_countries (
    store_id     UUID         NOT NULL,
    country_code CHAR(2)      NOT NULL,
    enabled      BOOLEAN      NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT store_shipping_countries_pkey                PRIMARY KEY (store_id, country_code),
    CONSTRAINT store_shipping_countries_store_id_fkey       FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT store_shipping_countries_country_code_check  CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE INDEX stores_status_idx ON commerce.stores (status);
CREATE INDEX shoppers_store_seen_idx ON commerce.shoppers (store_id, last_seen_at DESC, id DESC);
CREATE INDEX store_memberships_user_idx ON commerce.store_memberships (user_id, store_id);
CREATE INDEX channels_store_status_idx ON commerce.channels (store_id, status);
CREATE INDEX channel_publishable_keys_store_created_idx ON commerce.channel_publishable_keys (store_id, created_at DESC, id DESC);
CREATE INDEX channel_publishable_keys_channel_idx ON commerce.channel_publishable_keys (store_id, channel_id, id);
CREATE INDEX store_shipping_countries_enabled_idx ON commerce.store_shipping_countries (store_id) WHERE enabled;

CREATE FUNCTION commerce.authenticate_publishable_key (presented_public_key TEXT)
RETURNS TABLE (
    publishable_key_id UUID,
    store_id           UUID,
    channel_id         UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT
        publishable_key.id        AS publishable_key_id,
        publishable_key.store_id,
        channel.id               AS channel_id
    FROM
        commerce.channel_publishable_keys AS publishable_key
        INNER JOIN commerce.stores AS store
            ON store.id = publishable_key.store_id
        INNER JOIN commerce.channels AS channel
            ON channel.store_id = publishable_key.store_id
            AND channel.id = publishable_key.channel_id
            AND channel.status = 'active'
    WHERE
        publishable_key.public_key = presented_public_key
        AND publishable_key.revoked_at IS NULL
        AND store.status = 'active';
$$;

ALTER TABLE commerce.stores ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.shoppers ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.shoppers FORCE ROW LEVEL SECURITY;
ALTER TABLE commerce.store_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.channels ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.channel_publishable_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.store_shipping_countries ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.stores
    USING (id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.shoppers
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_directory ON commerce.stores
    FOR SELECT
    USING (
        EXISTS (
            SELECT 1
            FROM commerce.store_memberships AS membership
            WHERE membership.store_id = stores.id
              AND membership.user_id = nullif(current_setting('app.user_id', true), '')::uuid
        )
    );

CREATE POLICY store_isolation ON commerce.store_memberships
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_membership_directory ON commerce.store_memberships
    FOR SELECT
    USING (user_id = nullif(current_setting('app.user_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.channels
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.channel_publishable_keys
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.store_shipping_countries
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION commerce.authenticate_publishable_key (TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION commerce.authenticate_publishable_key (TEXT) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.stores,
       commerce.shoppers,
       commerce.store_memberships,
       commerce.channels,
       commerce.channel_publishable_keys,
       commerce.store_shipping_countries
    TO chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
