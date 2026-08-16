CREATE SCHEMA extensions;
CREATE SCHEMA identity;
CREATE SCHEMA integration;
CREATE SCHEMA merchant;
CREATE SCHEMA catalog;
CREATE SCHEMA pricing;
CREATE SCHEMA inventory;
CREATE SCHEMA sales;
CREATE SCHEMA payments;
CREATE SCHEMA fulfillment;
CREATE SCHEMA notification;
CREATE SCHEMA analytics;
CREATE SCHEMA search;
CREATE SCHEMA partman;

COMMENT ON SCHEMA extensions IS 'PostgreSQL extension-owned objects';
COMMENT ON SCHEMA identity IS 'Users, credentials, service accounts, and sessions';
COMMENT ON SCHEMA integration IS
    'Idempotency records, webhooks, outbox delivery, and external mappings';
COMMENT ON SCHEMA merchant IS
    'Merchant accounts, memberships, stores, channels, and domains';
COMMENT ON SCHEMA catalog IS
    'Products, variants, options, collections, media, and channel publication';
COMMENT ON SCHEMA pricing IS
    'Price lists, currency-specific prices, promotions, and tax classes';
COMMENT ON SCHEMA inventory IS
    'Locations, stock balances, append-only ledger entries, and reservations';
COMMENT ON SCHEMA sales IS
    'Carts, immutable checkout calculations, orders, returns, and exchanges';
COMMENT ON SCHEMA payments IS
    'Provider accounts, payment attempts, captures, and refunds';
COMMENT ON SCHEMA fulfillment IS
    'Partial fulfillments, shipments, tracking, and return logistics';
COMMENT ON SCHEMA notification IS
    'Semantic delivery requests, recipient policy, suppression, and delivery status';
COMMENT ON SCHEMA analytics IS
    'Canonical behavior events, consent evidence, attribution, and analytical delivery state';
COMMENT ON SCHEMA search IS
    'Rebuildable Store-isolated read models for storefront discovery';
COMMENT ON SCHEMA partman IS 'Objects owned by the pg_partman extension';

CREATE EXTENSION citext WITH SCHEMA extensions;
CREATE EXTENSION IF NOT EXISTS pg_partman WITH SCHEMA partman;
CREATE EXTENSION IF NOT EXISTS pg_cron;
CREATE EXTENSION IF NOT EXISTS pgmq;

CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');
CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'merchant_account', 'shopper');
CREATE TYPE merchant.merchant_account_status AS ENUM ('active', 'suspended', 'closed');
CREATE TYPE merchant.merchant_role AS ENUM (
    'owner',
    'administrator',
    'developer',
    'manager',
    'support'
);
CREATE TYPE merchant.store_status AS ENUM ('draft', 'active', 'archived');
CREATE TYPE merchant.sales_channel_kind AS ENUM (
    'web',
    'mobile',
    'point_of_sale',
    'marketplace',
    'custom'
);
CREATE TYPE merchant.sales_channel_status AS ENUM ('active', 'archived');
CREATE TYPE merchant.api_key_class AS ENUM ('publishable', 'secret');
CREATE TYPE merchant.api_key_mode AS ENUM ('test', 'live');
CREATE TYPE merchant.api_key_scope AS ENUM (
    'analytics:write',
    'catalog:read',
    'carts:write',
    'checkout:write',
    'orders:read',
    'customers:write',
    'mcp:tools'
);
CREATE TYPE catalog.product_status AS ENUM ('draft', 'active', 'archived');
CREATE TYPE catalog.variant_status AS ENUM ('active', 'archived');
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
CREATE TYPE sales.cart_status AS ENUM ('active', 'completed', 'abandoned');
CREATE TYPE sales.checkout_status AS ENUM ('pending', 'completed', 'expired');
CREATE TYPE sales.address_kind AS ENUM ('billing', 'shipping');
CREATE TYPE sales.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE sales.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');
CREATE TYPE sales.order_fulfillment_status AS ENUM (
    'unfulfilled',
    'partially_fulfilled',
    'fulfilled'
);
CREATE TYPE sales.order_delivery_status AS ENUM (
    'not_delivered',
    'partially_delivered',
    'delivered'
);
CREATE TYPE payments.payment_attempt_status AS ENUM (
    'pending',
    'authorized',
    'captured',
    'failed',
    'cancelled'
);
CREATE TYPE payments.refund_status AS ENUM ('pending', 'succeeded', 'failed');
CREATE TYPE integration.queue_status AS ENUM ('pending', 'processing', 'processed', 'dead_letter');
CREATE TYPE notification.email_delivery_status AS ENUM (
    'pending',
    'processing',
    'sent',
    'delivered',
    'bounced',
    'complained',
    'suppressed',
    'failed',
    'dead_letter'
);
CREATE TYPE notification.email_suppression_reason AS ENUM (
    'hard_bounce',
    'complaint',
    'provider_suppression',
    'manual'
);
CREATE TYPE analytics.event_source AS ENUM ('browser', 'server');
CREATE TYPE analytics.browser_event_name AS ENUM (
    'page_viewed',
    'product_viewed',
    'search_performed',
    'cart_line_added',
    'checkout_started',
    'engagement_heartbeat'
);
CREATE TYPE analytics.erasure_status AS ENUM ('pending', 'completed');
CREATE TYPE analytics.commerce_fact_name AS ENUM (
    'order_created',
    'payment_captured',
    'refund_succeeded',
    'fulfillment_shipped',
    'return_completed'
);
CREATE TYPE analytics.attribution_model AS ENUM ('first_touch', 'last_touch');
CREATE TYPE fulfillment.fulfillment_status AS ENUM (
    'pending',
    'shipped',
    'delivered',
    'cancelled'
);
CREATE TYPE fulfillment.return_status AS ENUM (
    'requested',
    'authorized',
    'received',
    'completed',
    'rejected'
);
CREATE TYPE fulfillment.return_disposition AS ENUM ('restock', 'discard');
CREATE TYPE fulfillment.shipping_service_status AS ENUM ('active', 'archived');

CREATE TABLE identity.users (
    id          UUID                    NOT NULL PRIMARY KEY,
    email       extensions.citext       NOT NULL UNIQUE,
    status      identity.user_status    NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT users_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    )
);

CREATE TABLE identity.magic_link_challenges (
    id            UUID                 NOT NULL PRIMARY KEY,
    email         extensions.citext    NOT NULL,
    token_digest  BYTEA                NOT NULL UNIQUE,
    expires_at    TIMESTAMPTZ          NOT NULL,
    consumed_at   TIMESTAMPTZ,
    created_at    TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT magic_link_challenges_token_digest_length_check CHECK (
        octet_length(token_digest) = 32
    ),
    CONSTRAINT magic_link_challenges_expiration_check CHECK (
        expires_at > created_at
    )
);

CREATE INDEX magic_link_challenges_email_created_idx
    ON identity.magic_link_challenges (email, created_at DESC);

CREATE TABLE identity.sessions (
    id            UUID           NOT NULL PRIMARY KEY,
    user_id       UUID           NOT NULL,
    token_digest  BYTEA          NOT NULL UNIQUE,
    expires_at    TIMESTAMPTZ    NOT NULL,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at  TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT sessions_token_digest_length_check CHECK (
        octet_length(token_digest) = 32
    ),
    CONSTRAINT sessions_expiration_check CHECK (
        expires_at > created_at
    )
);

CREATE INDEX sessions_user_expires_idx
    ON identity.sessions (user_id, expires_at DESC);

CREATE TABLE identity.passkey_credentials (
    id             UUID           NOT NULL PRIMARY KEY,
    user_id        UUID           NOT NULL,
    credential_id  BYTEA          NOT NULL UNIQUE,
    credential     JSONB          NOT NULL,
    name           TEXT           NOT NULL,
    created_at     TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at   TIMESTAMPTZ,

    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT passkey_credentials_credential_id_length_check CHECK (
        octet_length(credential_id) BETWEEN 16 AND 1024
    ),
    CONSTRAINT passkey_credentials_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    )
);

CREATE INDEX passkey_credentials_user_created_idx
    ON identity.passkey_credentials (user_id, created_at DESC);

CREATE TABLE integration.idempotency_records (
    id                   UUID                             NOT NULL PRIMARY KEY,
    scope                integration.idempotency_scope    NOT NULL,
    scope_id             UUID                             NOT NULL,
    operation            TEXT                             NOT NULL,
    idempotency_key      TEXT                             NOT NULL,
    request_fingerprint  BYTEA                            NOT NULL,
    response_status      SMALLINT,
    response_body        JSONB,
    completed_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (scope, scope_id, operation, idempotency_key),
    CONSTRAINT idempotency_records_operation_length_check CHECK (
        length(operation) BETWEEN 1 AND 120
    ),
    CONSTRAINT idempotency_records_key_length_check CHECK (
        octet_length(idempotency_key) BETWEEN 1 AND 255
    ),
    CONSTRAINT idempotency_records_request_fingerprint_length_check CHECK (
        octet_length(request_fingerprint) = 32
    ),
    CONSTRAINT idempotency_records_response_completion_check CHECK (
        (response_status IS NULL AND response_body IS NULL AND completed_at IS NULL)
        OR
        (response_status BETWEEN 200 AND 599 AND response_body IS NOT NULL AND completed_at IS NOT NULL)
    )
);

CREATE TABLE merchant.merchant_accounts (
    id            UUID                                NOT NULL PRIMARY KEY,
    slug          extensions.citext                   NOT NULL UNIQUE,
    display_name  TEXT                                NOT NULL,
    status        merchant.merchant_account_status    NOT NULL DEFAULT 'active',
    created_at    TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT merchant_accounts_slug_format_check CHECK (
        slug::text ~ '^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$'
    ),
    CONSTRAINT merchant_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE merchant.merchant_account_memberships (
    merchant_account_id  UUID                      NOT NULL,
    user_id              UUID                      NOT NULL,
    role                 merchant.merchant_role    NOT NULL,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (merchant_account_id, user_id),
    FOREIGN KEY (merchant_account_id)
        REFERENCES merchant.merchant_accounts(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE
);

CREATE INDEX merchant_account_memberships_user_idx
    ON merchant.merchant_account_memberships (user_id, merchant_account_id);

CREATE TABLE merchant.stores (
    id                   UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                     NOT NULL,
    code                 extensions.citext        NOT NULL,
    name                 TEXT                     NOT NULL,
    default_region       CHAR(2)                  NOT NULL DEFAULT 'US',
    default_currency     CHAR(3)                  NOT NULL DEFAULT 'USD',
    status               merchant.store_status    NOT NULL DEFAULT 'draft',
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, code),
    UNIQUE (merchant_account_id, id),
    FOREIGN KEY (merchant_account_id)
        REFERENCES merchant.merchant_accounts(id),
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
    )
);

CREATE INDEX stores_merchant_account_status_idx
    ON merchant.stores (merchant_account_id, status);

CREATE TABLE merchant.sales_channels (
    id                   UUID                              NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                              NOT NULL,
    store_id             UUID                              NOT NULL,
    code                 extensions.citext                 NOT NULL,
    name                 TEXT                              NOT NULL,
    kind                 merchant.sales_channel_kind       NOT NULL,
    status               merchant.sales_channel_status     NOT NULL DEFAULT 'active',
    is_default           BOOLEAN                           NOT NULL DEFAULT false,
    created_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, code),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT sales_channels_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT sales_channels_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE UNIQUE INDEX sales_channels_one_default_per_store_idx
    ON merchant.sales_channels (merchant_account_id, store_id)
    WHERE is_default;

CREATE INDEX sales_channels_store_status_idx
    ON merchant.sales_channels (merchant_account_id, store_id, status);

CREATE TABLE merchant.store_currencies (
    merchant_account_id  UUID       NOT NULL,
    store_id             UUID       NOT NULL,
    currency             CHAR(3)    NOT NULL,
    enabled              BOOLEAN    NOT NULL DEFAULT true,

    PRIMARY KEY (merchant_account_id, store_id, currency),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT store_currencies_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    )
);

CREATE TABLE merchant.store_domains (
    id                   UUID                 NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                 NOT NULL,
    store_id             UUID                 NOT NULL,
    hostname             extensions.citext    NOT NULL UNIQUE,
    verified_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT store_domains_hostname_lowercase_check CHECK (
        hostname::text = lower(hostname::text)
    )
);

CREATE INDEX store_domains_merchant_account_store_idx
    ON merchant.store_domains (merchant_account_id, store_id);

CREATE TABLE merchant.api_keys (
    id                   UUID                      NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                      NOT NULL,
    store_id             UUID                      NOT NULL,
    sales_channel_id     UUID,
    key_identifier       TEXT                      NOT NULL UNIQUE,
    secret_digest        BYTEA                     NOT NULL,
    display_suffix       CHAR(4)                   NOT NULL,
    name                 TEXT                      NOT NULL,
    class                merchant.api_key_class    NOT NULL,
    mode                 merchant.api_key_mode     NOT NULL,
    created_by_user_id   UUID                      NOT NULL,
    revoked_by_user_id   UUID,
    expires_at           TIMESTAMPTZ,
    last_used_at         TIMESTAMPTZ,
    revoked_at           TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
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

CREATE INDEX api_keys_store_created_idx
    ON merchant.api_keys (merchant_account_id, store_id, created_at DESC, id DESC);

CREATE TABLE merchant.api_key_scopes (
    merchant_account_id  UUID                      NOT NULL,
    api_key_id           UUID                      NOT NULL,
    scope                merchant.api_key_scope    NOT NULL,

    PRIMARY KEY (merchant_account_id, api_key_id, scope),
    FOREIGN KEY (merchant_account_id, api_key_id)
        REFERENCES merchant.api_keys(merchant_account_id, id) ON DELETE CASCADE
);

CREATE TABLE catalog.products (
    id                   UUID                       NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                       NOT NULL,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               catalog.product_status     NOT NULL DEFAULT 'draft',
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, handle),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT products_handle_format_check CHECK (
        handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'
    ),
    CONSTRAINT products_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT products_description_length_check CHECK (
        length(description) <= 100000
    )
);

CREATE INDEX products_store_status_created_idx
    ON catalog.products (merchant_account_id, store_id, status, created_at DESC, id DESC);

CREATE TABLE catalog.product_options (
    id                   UUID                 NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                 NOT NULL,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    name                 extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, product_id, name),
    UNIQUE (merchant_account_id, store_id, product_id, position),
    UNIQUE (merchant_account_id, store_id, product_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_id)
        REFERENCES catalog.products(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_options_name_length_check CHECK (
        length(trim(name::text)) BETWEEN 1 AND 80
    ),
    CONSTRAINT product_options_position_check CHECK (
        position BETWEEN 0 AND 9
    )
);

CREATE TABLE catalog.product_option_values (
    id                   UUID                 NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                 NOT NULL,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    option_id            UUID                 NOT NULL,
    value                extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, product_id, option_id, value),
    UNIQUE (merchant_account_id, store_id, product_id, option_id, position),
    UNIQUE (merchant_account_id, store_id, product_id, option_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_id, option_id)
        REFERENCES catalog.product_options(merchant_account_id, store_id, product_id, id)
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
    merchant_account_id  UUID                       NOT NULL,
    store_id             UUID                       NOT NULL,
    product_id           UUID                       NOT NULL,
    title                TEXT                       NOT NULL,
    sku                  extensions.citext,
    status               catalog.variant_status     NOT NULL DEFAULT 'active',
    requires_shipping    BOOLEAN                    NOT NULL DEFAULT true,
    track_inventory      BOOLEAN                    NOT NULL DEFAULT true,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, product_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_id)
        REFERENCES catalog.products(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variants_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT product_variants_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku::text)) BETWEEN 1 AND 64
    ),
    CONSTRAINT product_variants_sku_characters_check CHECK (
        sku IS NULL OR sku::text !~ '[[:cntrl:]]'
    )
);

CREATE UNIQUE INDEX product_variants_store_sku_key
    ON catalog.product_variants (merchant_account_id, store_id, sku)
    WHERE sku IS NOT NULL;

CREATE INDEX product_variants_product_status_idx
    ON catalog.product_variants (merchant_account_id, store_id, product_id, status);

CREATE TABLE catalog.variant_selected_options (
    merchant_account_id  UUID    NOT NULL,
    store_id             UUID    NOT NULL,
    product_id           UUID    NOT NULL,
    variant_id           UUID    NOT NULL,
    option_id            UUID    NOT NULL,
    option_value_id      UUID    NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, variant_id, option_id),
    UNIQUE (merchant_account_id, store_id, variant_id, option_value_id),
    FOREIGN KEY (merchant_account_id, store_id, product_id, variant_id)
        REFERENCES catalog.product_variants(merchant_account_id, store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, product_id, option_id, option_value_id)
        REFERENCES catalog.product_option_values(
            merchant_account_id,
            store_id,
            product_id,
            option_id,
            id
        ) ON DELETE CASCADE
);

CREATE TABLE catalog.product_publications (
    merchant_account_id  UUID        NOT NULL,
    store_id             UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (merchant_account_id, store_id, product_id, sales_channel_id),
    FOREIGN KEY (merchant_account_id, store_id, product_id)
        REFERENCES catalog.products(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id) ON DELETE CASCADE
);

CREATE INDEX product_publications_channel_product_idx
    ON catalog.product_publications (
        merchant_account_id,
        store_id,
        sales_channel_id,
        product_id
    );

CREATE TABLE pricing.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                         NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, code),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, currency)
        REFERENCES merchant.store_currencies(merchant_account_id, store_id, currency),
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

CREATE INDEX price_lists_store_activation_idx
    ON pricing.price_lists (
        merchant_account_id,
        store_id,
        status,
        currency,
        starts_at,
        ends_at
    );

CREATE TABLE pricing.prices (
    id                   UUID         NOT NULL PRIMARY KEY,
    merchant_account_id  UUID         NOT NULL,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, price_list_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, price_list_id)
        REFERENCES pricing.price_lists(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, product_variant_id)
        REFERENCES catalog.product_variants(merchant_account_id, store_id, id),
    CONSTRAINT prices_amount_nonnegative_check CHECK (
        amount_minor >= 0
    )
);

CREATE INDEX prices_variant_lookup_idx
    ON pricing.prices (
        merchant_account_id,
        store_id,
        product_variant_id,
        price_list_id
    );

CREATE TABLE pricing.tax_rules (
    id                    UUID                    NOT NULL PRIMARY KEY,
    merchant_account_id   UUID                    NOT NULL,
    store_id              UUID                    NOT NULL,
    code                  TEXT                    NOT NULL,
    name                  TEXT                    NOT NULL,
    country_code          CHAR(2)                 NOT NULL,
    rate_basis_points     INTEGER                 NOT NULL,
    status                pricing.tax_rule_status NOT NULL DEFAULT 'active',
    created_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, code),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT tax_rules_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT tax_rules_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_rules_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT tax_rules_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE UNIQUE INDEX tax_rules_active_country_key
    ON pricing.tax_rules (merchant_account_id, store_id, country_code)
    WHERE status = 'active';

CREATE INDEX tax_rules_store_status_idx
    ON pricing.tax_rules (merchant_account_id, store_id, status, created_at, id);

CREATE TABLE pricing.promotions (
    id                            UUID                         NOT NULL PRIMARY KEY,
    merchant_account_id           UUID                         NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, handle),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, currency)
        REFERENCES merchant.store_currencies(merchant_account_id, store_id, currency),
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

CREATE UNIQUE INDEX promotions_active_redemption_code_key
    ON pricing.promotions (merchant_account_id, store_id, redemption_code)
    WHERE status = 'active' AND redemption_code IS NOT NULL;

CREATE INDEX promotions_checkout_lookup_idx
    ON pricing.promotions (
        merchant_account_id, store_id, currency, status, trigger, priority, id
    );

CREATE TABLE inventory.inventory_locations (
    id                   UUID                                    NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                                    NOT NULL,
    store_id             UUID                                    NOT NULL,
    code                 extensions.citext                       NOT NULL,
    name                 TEXT                                    NOT NULL,
    status               inventory.inventory_location_status     NOT NULL DEFAULT 'active',
    created_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, code),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT inventory_locations_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT inventory_locations_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE INDEX inventory_locations_store_status_idx
    ON inventory.inventory_locations (merchant_account_id, store_id, status, created_at, id);

CREATE TABLE inventory.stock_items (
    id                    UUID        NOT NULL PRIMARY KEY,
    merchant_account_id   UUID        NOT NULL,
    store_id              UUID        NOT NULL,
    inventory_location_id UUID        NOT NULL,
    product_variant_id    UUID        NOT NULL,
    on_hand_quantity      BIGINT      NOT NULL DEFAULT 0,
    reserved_quantity     BIGINT      NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, inventory_location_id, product_variant_id),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, inventory_location_id)
        REFERENCES inventory.inventory_locations(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_variant_id)
        REFERENCES catalog.product_variants(merchant_account_id, store_id, id),
    CONSTRAINT stock_items_on_hand_nonnegative_check CHECK (on_hand_quantity >= 0),
    CONSTRAINT stock_items_reserved_range_check CHECK (
        reserved_quantity >= 0 AND reserved_quantity <= on_hand_quantity
    )
);

CREATE INDEX stock_items_variant_availability_idx
    ON inventory.stock_items (
        merchant_account_id,
        store_id,
        product_variant_id,
        inventory_location_id
    );

CREATE TABLE inventory.inventory_reservations (
    id                   UUID                                      NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                                      NOT NULL,
    store_id             UUID                                      NOT NULL,
    sales_channel_id     UUID                                      NOT NULL,
    status               inventory.inventory_reservation_status    NOT NULL DEFAULT 'active',
    expires_at           TIMESTAMPTZ                               NOT NULL,
    closed_at            TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    CONSTRAINT inventory_reservations_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT inventory_reservations_closure_check CHECK (
        (status = 'active' AND closed_at IS NULL)
        OR (status <> 'active' AND closed_at IS NOT NULL)
    )
);

CREATE INDEX inventory_reservations_expiration_idx
    ON inventory.inventory_reservations (
        merchant_account_id,
        store_id,
        status,
        expires_at,
        id
    );

CREATE TABLE inventory.inventory_reservation_lines (
    merchant_account_id  UUID    NOT NULL,
    store_id             UUID    NOT NULL,
    reservation_id       UUID    NOT NULL,
    stock_item_id        UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             BIGINT  NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, reservation_id, stock_item_id),
    FOREIGN KEY (merchant_account_id, store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(merchant_account_id, store_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, stock_item_id)
        REFERENCES inventory.stock_items(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_variant_id)
        REFERENCES catalog.product_variants(merchant_account_id, store_id, id),
    CONSTRAINT inventory_reservation_lines_quantity_positive_check CHECK (quantity > 0)
);

CREATE INDEX inventory_reservation_lines_stock_item_idx
    ON inventory.inventory_reservation_lines (
        merchant_account_id,
        store_id,
        stock_item_id,
        reservation_id
    );

CREATE TABLE inventory.stock_ledger_entries (
    id                           UUID                        NOT NULL PRIMARY KEY,
    merchant_account_id          UUID                        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, stock_item_id)
        REFERENCES inventory.stock_items(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(merchant_account_id, store_id, id),
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

CREATE INDEX stock_ledger_entries_stock_item_created_idx
    ON inventory.stock_ledger_entries (
        merchant_account_id,
        store_id,
        stock_item_id,
        created_at DESC,
        id DESC
    );

ALTER TABLE pricing.price_lists
    ADD UNIQUE (merchant_account_id, store_id, id, currency);

CREATE TABLE sales.customers (
    id                  UUID              NOT NULL PRIMARY KEY,
    merchant_account_id UUID              NOT NULL,
    store_id            UUID              NOT NULL,
    user_id             UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,
    created_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, user_id),
    UNIQUE (merchant_account_id, store_id, email),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES identity.users(id),
    CONSTRAINT customers_email_length_check CHECK (length(trim(email::text)) BETWEEN 3 AND 320),
    CONSTRAINT customers_phone_format_check CHECK (phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$')
);

CREATE INDEX customers_store_created_idx
    ON sales.customers (merchant_account_id, store_id, created_at DESC, id DESC);

CREATE TABLE sales.customer_addresses (
    id                   UUID     NOT NULL PRIMARY KEY,
    merchant_account_id  UUID     NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, customer_id, id),
    CONSTRAINT customer_addresses_customer_label_key
        UNIQUE (merchant_account_id, store_id, customer_id, label),
    CONSTRAINT customer_addresses_customer_fkey
        FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_addresses_label_length_check CHECK (length(trim(label)) BETWEEN 1 AND 64),
    CONSTRAINT customer_addresses_full_name_length_check CHECK (length(trim(full_name)) BETWEEN 1 AND 200),
    CONSTRAINT customer_addresses_company_length_check CHECK (company IS NULL OR length(trim(company)) BETWEEN 1 AND 200),
    CONSTRAINT customer_addresses_line1_length_check CHECK (length(trim(address_line1)) BETWEEN 1 AND 255),
    CONSTRAINT customer_addresses_line2_length_check CHECK (address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255),
    CONSTRAINT customer_addresses_locality_length_check CHECK (length(trim(locality)) BETWEEN 1 AND 100),
    CONSTRAINT customer_addresses_area_length_check CHECK (administrative_area IS NULL OR length(trim(administrative_area)) BETWEEN 1 AND 100),
    CONSTRAINT customer_addresses_postal_code_length_check CHECK (postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32),
    CONSTRAINT customer_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE INDEX customer_addresses_customer_idx
    ON sales.customer_addresses (merchant_account_id, store_id, customer_id, created_at, id);

CREATE TABLE sales.customer_shopper_links (
    merchant_account_id UUID        NOT NULL,
    store_id            UUID        NOT NULL,
    customer_id         UUID        NOT NULL,
    shopper_id          UUID        NOT NULL,
    sales_channel_id    UUID        NOT NULL,
    linked_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (merchant_account_id, store_id, shopper_id),
    CONSTRAINT customer_shopper_links_customer_fkey
        FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_shopper_links_channel_fkey
        FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id) ON DELETE CASCADE
);

CREATE INDEX customer_shopper_links_history_idx
    ON sales.customer_shopper_links (
        merchant_account_id, store_id, customer_id, sales_channel_id, shopper_id
    );

CREATE TABLE sales.carts (
    id                   UUID                NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                NOT NULL,
    store_id             UUID                NOT NULL,
    sales_channel_id     UUID                NOT NULL,
    shopper_id           UUID                NOT NULL,
    customer_id          UUID,
    price_list_id        UUID                NOT NULL,
    currency             CHAR(3)             NOT NULL,
    status               sales.cart_status   NOT NULL DEFAULT 'active',
    version              BIGINT              NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, id, shopper_id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(merchant_account_id, store_id, id, currency),
    CONSTRAINT carts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT carts_version_nonnegative_check CHECK (version >= 0)
);

CREATE INDEX carts_channel_updated_idx
    ON sales.carts (
        merchant_account_id,
        store_id,
        sales_channel_id,
        status,
        updated_at DESC,
        id DESC
    );

CREATE TABLE sales.cart_lines (
    merchant_account_id     UUID        NOT NULL,
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

    PRIMARY KEY (merchant_account_id, store_id, cart_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, cart_id)
        REFERENCES sales.carts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, product_id, product_variant_id)
        REFERENCES catalog.product_variants(merchant_account_id, store_id, product_id, id),
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

CREATE INDEX cart_lines_variant_lookup_idx
    ON sales.cart_lines (merchant_account_id, store_id, product_variant_id, cart_id);

CREATE TABLE sales.checkouts (
    id                     UUID                    NOT NULL PRIMARY KEY,
    merchant_account_id    UUID                    NOT NULL,
    store_id               UUID                    NOT NULL,
    cart_id                UUID                    NOT NULL,
    shopper_id             UUID                    NOT NULL,
    customer_id            UUID,
    sales_channel_id       UUID                    NOT NULL,
    price_list_id          UUID                    NOT NULL,
    inventory_reservation_id UUID,
    currency               CHAR(3)                 NOT NULL,
    status                 sales.checkout_status   NOT NULL DEFAULT 'pending',
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

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, id, shopper_id),
    UNIQUE (merchant_account_id, store_id, cart_id),
    UNIQUE (merchant_account_id, store_id, inventory_reservation_id),
    FOREIGN KEY (merchant_account_id, store_id, cart_id, shopper_id)
        REFERENCES sales.carts(merchant_account_id, store_id, id, shopper_id),
    FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(merchant_account_id, store_id, id, currency),
    FOREIGN KEY (merchant_account_id, store_id, inventory_reservation_id)
        REFERENCES inventory.inventory_reservations(merchant_account_id, store_id, id),
    CONSTRAINT checkouts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
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

CREATE INDEX checkouts_channel_created_idx
    ON sales.checkouts (
        merchant_account_id,
        store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX checkouts_expiry_claim_idx
    ON sales.checkouts (expires_at, id)
    WHERE status = 'pending';

COMMENT ON INDEX sales.checkouts_expiry_claim_idx IS
    'Supports the cross-tenant SECURITY DEFINER expiry scheduler claim path';

CREATE TABLE sales.checkout_contacts (
    merchant_account_id UUID              NOT NULL,
    store_id            UUID              NOT NULL,
    checkout_id         UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (merchant_account_id, store_id, checkout_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT checkout_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT checkout_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE TABLE sales.checkout_addresses (
    merchant_account_id  UUID               NOT NULL,
    store_id             UUID               NOT NULL,
    checkout_id          UUID               NOT NULL,
    kind                 sales.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, checkout_id, kind),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id) ON DELETE CASCADE,
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

CREATE TABLE sales.checkout_tax_calculations (
    merchant_account_id UUID    NOT NULL,
    store_id            UUID    NOT NULL,
    checkout_id         UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, checkout_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, tax_rule_id)
        REFERENCES pricing.tax_rules(merchant_account_id, store_id, id),
    CONSTRAINT checkout_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT checkout_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE sales.checkout_promotion_calculations (
    merchant_account_id           UUID                      NOT NULL,
    store_id                      UUID                      NOT NULL,
    checkout_id                   UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       pricing.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, checkout_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, promotion_id)
        REFERENCES pricing.promotions(merchant_account_id, store_id, id),
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

CREATE TABLE sales.checkout_lines (
    merchant_account_id      UUID        NOT NULL,
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

    PRIMARY KEY (merchant_account_id, store_id, checkout_id, position),
    UNIQUE (merchant_account_id, store_id, checkout_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id),
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

CREATE TABLE sales.orders (
    id                       UUID                  NOT NULL PRIMARY KEY,
    merchant_account_id      UUID                  NOT NULL,
    store_id                 UUID                  NOT NULL,
    sales_channel_id         UUID                  NOT NULL,
    checkout_id              UUID                  NOT NULL,
    shopper_id               UUID                  NOT NULL,
    customer_id              UUID,
    inventory_reservation_id UUID,
    price_list_id            UUID                  NOT NULL,
    currency                 CHAR(3)               NOT NULL,
    status                   sales.order_status    NOT NULL DEFAULT 'pending',
    fulfillment_status       sales.order_fulfillment_status NOT NULL DEFAULT 'unfulfilled',
    delivery_status          sales.order_delivery_status NOT NULL DEFAULT 'not_delivered',
    subtotal_amount_minor    BIGINT                NOT NULL,
    discount_amount_minor    BIGINT                NOT NULL,
    tax_amount_minor         BIGINT                NOT NULL,
    tax_inclusive            BOOLEAN               NOT NULL,
    shipping_amount_minor    BIGINT                NOT NULL,
    total_amount_minor       BIGINT                NOT NULL,
    created_at               TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, id, shopper_id),
    UNIQUE (merchant_account_id, store_id, checkout_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id, shopper_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id, shopper_id),
    FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(merchant_account_id, store_id, id, currency),
    FOREIGN KEY (merchant_account_id, store_id, inventory_reservation_id)
        REFERENCES inventory.inventory_reservations(merchant_account_id, store_id, id),
    CONSTRAINT orders_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
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

CREATE INDEX orders_channel_created_idx
    ON sales.orders (
        merchant_account_id,
        store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE sales.order_contacts (
    merchant_account_id UUID              NOT NULL,
    store_id            UUID              NOT NULL,
    order_id            UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (merchant_account_id, store_id, order_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT order_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE INDEX orders_customer_created_idx
    ON sales.orders (
        merchant_account_id, store_id, customer_id, created_at DESC, id DESC
    ) WHERE customer_id IS NOT NULL;

CREATE TABLE sales.order_addresses (
    merchant_account_id  UUID               NOT NULL,
    store_id             UUID               NOT NULL,
    order_id             UUID               NOT NULL,
    kind                 sales.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, order_id, kind),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id) ON DELETE CASCADE,
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

CREATE TABLE sales.order_tax_calculations (
    merchant_account_id UUID    NOT NULL,
    store_id            UUID    NOT NULL,
    order_id            UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, order_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, tax_rule_id)
        REFERENCES pricing.tax_rules(merchant_account_id, store_id, id),
    CONSTRAINT order_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT order_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE sales.order_promotion_calculations (
    merchant_account_id           UUID                      NOT NULL,
    store_id                      UUID                      NOT NULL,
    order_id                      UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       pricing.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, order_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, promotion_id)
        REFERENCES pricing.promotions(merchant_account_id, store_id, id),
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

CREATE TABLE sales.order_lines (
    merchant_account_id      UUID        NOT NULL,
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

    PRIMARY KEY (merchant_account_id, store_id, order_id, position),
    UNIQUE (merchant_account_id, store_id, order_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id),
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

CREATE TABLE sales.order_transitions (
    id                   UUID                         NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                         NOT NULL,
    store_id             UUID                         NOT NULL,
    order_id             UUID                         NOT NULL,
    from_status          sales.order_status,
    to_status            sales.order_status           NOT NULL,
    kind                 sales.order_transition_kind NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                  NOT NULL,

    UNIQUE (merchant_account_id, store_id, order_id, id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id),
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT order_transitions_shape_check CHECK (
        (kind = 'created' AND from_status IS NULL AND to_status = 'pending')
        OR (kind = 'confirmed' AND from_status = 'pending' AND to_status = 'confirmed')
        OR (kind = 'cancelled' AND from_status = 'pending' AND to_status = 'cancelled')
    )
);

CREATE INDEX order_transitions_order_time_idx
    ON sales.order_transitions (
        merchant_account_id,
        store_id,
        order_id,
        occurred_at,
        id
    );

ALTER TABLE sales.orders
    ADD UNIQUE (merchant_account_id, store_id, id, currency);

CREATE TABLE payments.provider_accounts (
    id                         UUID        NOT NULL PRIMARY KEY,
    merchant_account_id        UUID        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    CONSTRAINT provider_accounts_store_provider_key
        UNIQUE (merchant_account_id, store_id, provider),
    UNIQUE (provider, external_account_reference),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
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
    ),
    CONSTRAINT provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
    ),
    CONSTRAINT provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_webhook_reference_check CHECK (
        webhook_secret_reference IS NULL
        OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
    ),
    CONSTRAINT provider_accounts_previous_webhook_reference_check CHECK (
        previous_webhook_secret_reference IS NULL
        OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
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

CREATE INDEX provider_accounts_store_created_idx
    ON payments.provider_accounts (merchant_account_id, store_id, created_at DESC, id DESC);

CREATE INDEX provider_accounts_readiness_due_idx
    ON payments.provider_accounts (readiness_reconcile_at, id)
    WHERE enabled;

CREATE TABLE payments.payment_attempts (
    id                     UUID                            NOT NULL PRIMARY KEY,
    merchant_account_id    UUID                            NOT NULL,
    store_id               UUID                            NOT NULL,
    order_id               UUID                            NOT NULL,
    shopper_id             UUID                            NOT NULL,
    provider_account_id    UUID                            NOT NULL,
    amount_minor           BIGINT                          NOT NULL,
    currency               CHAR(3)                         NOT NULL,
    status                 payments.payment_attempt_status NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, id, shopper_id),
    UNIQUE (merchant_account_id, store_id, id, currency),
    UNIQUE (provider_account_id, provider_reference),
    FOREIGN KEY (merchant_account_id, store_id, order_id, shopper_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id, shopper_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id, currency)
        REFERENCES sales.orders(merchant_account_id, store_id, id, currency),
    FOREIGN KEY (merchant_account_id, store_id, provider_account_id)
        REFERENCES payments.provider_accounts(merchant_account_id, store_id, id),
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

CREATE INDEX payment_attempts_order_created_idx
    ON payments.payment_attempts (
        merchant_account_id,
        store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE payments.refunds (
    id                     UUID                    NOT NULL PRIMARY KEY,
    merchant_account_id    UUID                    NOT NULL,
    store_id               UUID                    NOT NULL,
    payment_attempt_id     UUID                    NOT NULL,
    amount_minor           BIGINT                  NOT NULL,
    currency               CHAR(3)                 NOT NULL,
    status                 payments.refund_status  NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (payment_attempt_id, provider_reference),
    FOREIGN KEY (merchant_account_id, store_id, payment_attempt_id, currency)
        REFERENCES payments.payment_attempts(merchant_account_id, store_id, id, currency),
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

CREATE INDEX refunds_attempt_created_idx
    ON payments.refunds (
        merchant_account_id,
        store_id,
        payment_attempt_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE fulfillment.shipping_services (
    id                         UUID                                NOT NULL PRIMARY KEY,
    merchant_account_id        UUID                                NOT NULL,
    store_id                   UUID                                NOT NULL,
    code                       TEXT                                NOT NULL,
    name                       TEXT                                NOT NULL,
    amount_minor               BIGINT                              NOT NULL,
    currency                   CHAR(3)                             NOT NULL,
    estimated_min_days         SMALLINT                            NOT NULL,
    estimated_max_days         SMALLINT                            NOT NULL,
    status                     fulfillment.shipping_service_status NOT NULL DEFAULT 'active',
    created_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, code),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
    FOREIGN KEY (merchant_account_id, store_id, currency)
        REFERENCES merchant.store_currencies(merchant_account_id, store_id, currency),
    CONSTRAINT shipping_services_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT shipping_services_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT shipping_services_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT shipping_services_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT shipping_services_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE INDEX shipping_services_quote_idx
    ON fulfillment.shipping_services (
        merchant_account_id,
        store_id,
        currency,
        status,
        id
    );

CREATE TABLE fulfillment.shipping_provider_accounts (
    id                                   UUID        NOT NULL PRIMARY KEY,
    merchant_account_id                  UUID        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    CONSTRAINT shipping_provider_accounts_store_provider_key
        UNIQUE (merchant_account_id, store_id, provider),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT shipping_provider_accounts_provider_format_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT shipping_provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_credential_reference_check CHECK (
        credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
    ),
    CONSTRAINT shipping_provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
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

CREATE INDEX shipping_provider_accounts_store_created_idx
    ON fulfillment.shipping_provider_accounts (
        merchant_account_id,
        store_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE fulfillment.shipping_service_regions (
    merchant_account_id UUID    NOT NULL,
    store_id            UUID    NOT NULL,
    shipping_service_id UUID    NOT NULL,
    country_code        CHAR(2) NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, shipping_service_id, country_code),
    FOREIGN KEY (merchant_account_id, store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(merchant_account_id, store_id, id),
    CONSTRAINT shipping_service_regions_country_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE INDEX shipping_service_regions_quote_idx
    ON fulfillment.shipping_service_regions (
        merchant_account_id,
        store_id,
        country_code,
        shipping_service_id
    );

CREATE TABLE sales.checkout_shipping_selections (
    merchant_account_id UUID        NOT NULL,
    store_id            UUID        NOT NULL,
    checkout_id         UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, checkout_id),
    FOREIGN KEY (merchant_account_id, store_id, checkout_id)
        REFERENCES sales.checkouts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(merchant_account_id, store_id, id),
    CONSTRAINT checkout_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT checkout_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT checkout_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE sales.order_shipping_selections (
    merchant_account_id UUID        NOT NULL,
    store_id            UUID        NOT NULL,
    order_id            UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, order_id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(merchant_account_id, store_id, id),
    CONSTRAINT order_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT order_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT order_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE fulfillment.fulfillments (
    id                   UUID                           NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                           NOT NULL,
    store_id             UUID                           NOT NULL,
    order_id             UUID                           NOT NULL,
    status               fulfillment.fulfillment_status NOT NULL DEFAULT 'pending',
    carrier              TEXT,
    tracking_number      TEXT,
    shipped_at           TIMESTAMPTZ,
    delivered_at         TIMESTAMPTZ,
    cancelled_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, carrier, tracking_number),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id),
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

CREATE INDEX fulfillments_order_created_idx
    ON fulfillment.fulfillments (
        merchant_account_id,
        store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE fulfillment.fulfillment_lines (
    merchant_account_id  UUID    NOT NULL,
    store_id             UUID    NOT NULL,
    fulfillment_id       UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             INTEGER NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, fulfillment_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(merchant_account_id, store_id, id),
    CONSTRAINT fulfillment_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999)
);

CREATE INDEX fulfillment_lines_variant_idx
    ON fulfillment.fulfillment_lines (
        merchant_account_id,
        store_id,
        product_variant_id,
        fulfillment_id
    );

CREATE TABLE fulfillment.shipping_quote_requests (
    id                    UUID        NOT NULL PRIMARY KEY,
    merchant_account_id   UUID        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, idempotency_key),
    FOREIGN KEY (merchant_account_id, store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, provider_account_id)
        REFERENCES fulfillment.shipping_provider_accounts(merchant_account_id, store_id, id),
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

CREATE INDEX shipping_quote_requests_fulfillment_created_idx
    ON fulfillment.shipping_quote_requests (
        merchant_account_id,
        store_id,
        fulfillment_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE fulfillment.shipping_rate_quotes (
    id                            UUID        NOT NULL PRIMARY KEY,
    merchant_account_id           UUID        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, quote_request_id, provider_rate_reference),
    FOREIGN KEY (merchant_account_id, store_id, quote_request_id)
        REFERENCES fulfillment.shipping_quote_requests(merchant_account_id, store_id, id),
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

CREATE INDEX shipping_rate_quotes_request_expiry_idx
    ON fulfillment.shipping_rate_quotes (
        merchant_account_id,
        store_id,
        quote_request_id,
        expires_at,
        id
    );

CREATE TABLE fulfillment.shipping_labels (
    id                              UUID        NOT NULL PRIMARY KEY,
    merchant_account_id             UUID        NOT NULL,
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

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, fulfillment_id),
    CONSTRAINT shipping_labels_purchase_idempotency_key
        UNIQUE (merchant_account_id, store_id, purchase_idempotency_key),
    CONSTRAINT shipping_labels_cancellation_idempotency_key
        UNIQUE (merchant_account_id, store_id, cancellation_idempotency_key),
    UNIQUE (carrier, tracking_number),
    FOREIGN KEY (merchant_account_id, store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, provider_account_id)
        REFERENCES fulfillment.shipping_provider_accounts(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, rate_quote_id)
        REFERENCES fulfillment.shipping_rate_quotes(merchant_account_id, store_id, id),
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

CREATE INDEX shipping_labels_tracking_due_idx
    ON fulfillment.shipping_labels (next_tracking_refresh_at, id)
    WHERE purchase_state = 'purchased' AND next_tracking_refresh_at IS NOT NULL;

CREATE INDEX shipping_labels_cancellation_due_idx
    ON fulfillment.shipping_labels (cancellation_reconcile_at, id)
    WHERE cancellation_status = 'submitted' AND cancellation_reconcile_at IS NOT NULL;

CREATE TABLE fulfillment.returns (
    id                   UUID                      NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                      NOT NULL,
    store_id             UUID                      NOT NULL,
    order_id             UUID                      NOT NULL,
    status               fulfillment.return_status NOT NULL DEFAULT 'requested',
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

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, order_id, currency)
        REFERENCES sales.orders(merchant_account_id, store_id, id, currency),
    FOREIGN KEY (merchant_account_id, store_id, refund_id)
        REFERENCES payments.refunds(merchant_account_id, store_id, id),
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

CREATE INDEX returns_order_created_idx
    ON fulfillment.returns (
        merchant_account_id,
        store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE TABLE fulfillment.return_lines (
    merchant_account_id  UUID                           NOT NULL,
    store_id             UUID                           NOT NULL,
    return_id            UUID                           NOT NULL,
    product_variant_id   UUID                           NOT NULL,
    inventory_location_id UUID,
    quantity             INTEGER                        NOT NULL,
    refund_amount_minor  BIGINT                         NOT NULL,
    disposition          fulfillment.return_disposition,

    PRIMARY KEY (merchant_account_id, store_id, return_id, product_variant_id),
    FOREIGN KEY (merchant_account_id, store_id, return_id)
        REFERENCES fulfillment.returns(merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, inventory_location_id)
        REFERENCES inventory.inventory_locations(merchant_account_id, store_id, id),
    CONSTRAINT return_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT return_lines_refund_amount_nonnegative_check CHECK (refund_amount_minor >= 0),
    CONSTRAINT return_lines_restock_location_check CHECK (
        disposition <> 'restock' OR inventory_location_id IS NOT NULL
    )
);

CREATE INDEX return_lines_variant_idx
    ON fulfillment.return_lines (
        merchant_account_id,
        store_id,
        product_variant_id,
        return_id
    );

CREATE TABLE notification.email_deliveries (
    id                       UUID                               NOT NULL PRIMARY KEY,
    merchant_account_id      UUID                               NOT NULL,
    store_id                 UUID                               NOT NULL,
    semantic_event_id        UUID                               NOT NULL,
    semantic_event_type      TEXT                               NOT NULL,
    recipient_email          extensions.citext                  NOT NULL,
    template_key             TEXT                               NOT NULL,
    template_version         INTEGER                            NOT NULL,
    template_payload         JSONB                              NOT NULL,
    provider                 TEXT                               NOT NULL DEFAULT 'resend',
    provider_message_id      TEXT,
    delivery_status          notification.email_delivery_status NOT NULL DEFAULT 'pending',
    attempts                 INTEGER                            NOT NULL DEFAULT 0,
    available_at             TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by                UUID,
    locked_at                TIMESTAMPTZ,
    sent_at                  TIMESTAMPTZ,
    delivered_at             TIMESTAMPTZ,
    last_error               TEXT,
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, semantic_event_id),
    UNIQUE (provider, provider_message_id),
    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT email_deliveries_semantic_event_type_check CHECK (
        semantic_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT email_deliveries_recipient_length_check CHECK (
        length(recipient_email::text) BETWEEN 3 AND 320
    ),
    CONSTRAINT email_deliveries_template_key_check CHECK (
        template_key ~ '^[a-z][a-z0-9_]{0,63}$'
    ),
    CONSTRAINT email_deliveries_template_version_check CHECK (template_version > 0),
    CONSTRAINT email_deliveries_template_payload_check CHECK (
        jsonb_typeof(template_payload) = 'object'
        AND octet_length(template_payload::text) <= 16384
    ),
    CONSTRAINT email_deliveries_provider_check CHECK (
        length(trim(provider)) BETWEEN 1 AND 50
    ),
    CONSTRAINT email_deliveries_provider_message_id_check CHECK (
        provider_message_id IS NULL OR length(provider_message_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT email_deliveries_attempts_check CHECK (attempts >= 0),
    CONSTRAINT email_deliveries_lease_check CHECK (
        (delivery_status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (delivery_status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    ),
    CONSTRAINT email_deliveries_sent_check CHECK (
        (delivery_status IN ('sent', 'delivered', 'bounced', 'complained')
            AND provider_message_id IS NOT NULL AND sent_at IS NOT NULL)
        OR delivery_status NOT IN ('sent', 'delivered', 'bounced', 'complained')
    ),
    CONSTRAINT email_deliveries_delivered_check CHECK (
        (delivery_status = 'delivered' AND delivered_at IS NOT NULL)
        OR (delivery_status <> 'delivered' AND delivered_at IS NULL)
    )
);

CREATE INDEX email_deliveries_claim_idx
    ON notification.email_deliveries (delivery_status, available_at, created_at, id)
    WHERE delivery_status IN ('pending', 'processing');

CREATE INDEX email_deliveries_recipient_idx
    ON notification.email_deliveries (
        merchant_account_id,
        store_id,
        recipient_email,
        created_at DESC,
        id DESC
    );

CREATE TABLE notification.email_suppressions (
    id                    UUID                                      NOT NULL PRIMARY KEY,
    merchant_account_id   UUID                                      NOT NULL,
    store_id              UUID                                      NOT NULL,
    recipient_email       extensions.citext                         NOT NULL,
    suppression_reason    notification.email_suppression_reason     NOT NULL,
    source_delivery_id    UUID,
    created_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, recipient_email),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, source_delivery_id)
        REFERENCES notification.email_deliveries(merchant_account_id, store_id, id),
    CONSTRAINT email_suppressions_recipient_length_check CHECK (
        length(recipient_email::text) BETWEEN 3 AND 320
    )
);

CREATE TABLE notification.webhook_events (
    id                    UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id   UUID                     NOT NULL,
    store_id              UUID                     NOT NULL,
    delivery_id           UUID                     NOT NULL,
    provider              TEXT                     NOT NULL,
    provider_event_id     TEXT                     NOT NULL,
    provider_event_type   TEXT                     NOT NULL,
    payload               JSONB                    NOT NULL,
    received_at           TIMESTAMPTZ              NOT NULL,
    processed_at          TIMESTAMPTZ,
    created_at            TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider, provider_event_id),
    FOREIGN KEY (merchant_account_id, store_id, delivery_id)
        REFERENCES notification.email_deliveries(merchant_account_id, store_id, id),
    CONSTRAINT notification_webhook_events_provider_check CHECK (
        length(trim(provider)) BETWEEN 1 AND 50
    ),
    CONSTRAINT notification_webhook_events_event_id_check CHECK (
        length(provider_event_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT notification_webhook_events_event_type_check CHECK (
        provider_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT notification_webhook_events_payload_check CHECK (
        jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 65536
    )
);

CREATE INDEX notification_webhook_events_delivery_idx
    ON notification.webhook_events (
        merchant_account_id,
        store_id,
        delivery_id,
        received_at,
        id
    );

CREATE TABLE analytics.store_policy_versions (
    id                              UUID        NOT NULL PRIMARY KEY,
    merchant_account_id             UUID        NOT NULL,
    store_id                        UUID        NOT NULL,
    version                         INTEGER     NOT NULL,
    behavior_collection_enabled     BOOLEAN     NOT NULL,
    advertising_exports_enabled     BOOLEAN     NOT NULL,
    identity_linking_enabled        BOOLEAN     NOT NULL,
    raw_event_retention_days        SMALLINT    NOT NULL,
    created_by                      UUID        NOT NULL,
    effective_at                    TIMESTAMPTZ NOT NULL,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, version),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (created_by) REFERENCES identity.users(id),
    CONSTRAINT store_policy_versions_version_check CHECK (version BETWEEN 1 AND 2147483647),
    CONSTRAINT store_policy_versions_retention_check CHECK (
        raw_event_retention_days BETWEEN 1 AND 400
    )
);

CREATE INDEX store_policy_versions_current_idx
    ON analytics.store_policy_versions (
        merchant_account_id,
        store_id,
        effective_at DESC,
        version DESC
    );

CREATE TABLE analytics.identity_links (
    id                         UUID        NOT NULL PRIMARY KEY,
    merchant_account_id        UUID        NOT NULL,
    store_id                   UUID        NOT NULL,
    anonymous_id               UUID        NOT NULL,
    customer_id                UUID        NOT NULL,
    consent_policy_version     TEXT        NOT NULL,
    collection_policy_version  TEXT        NOT NULL,
    linked_at                  TIMESTAMPTZ NOT NULL,
    retention_expires_at       TIMESTAMPTZ NOT NULL,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, anonymous_id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, customer_id)
        REFERENCES sales.customers(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT identity_links_anonymous_id_check CHECK (
        anonymous_id <> '00000000-0000-0000-0000-000000000000'::UUID
    ),
    CONSTRAINT identity_links_consent_policy_check CHECK (
        consent_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT identity_links_collection_policy_check CHECK (
        collection_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT identity_links_retention_check CHECK (
        retention_expires_at > linked_at
        AND retention_expires_at <= linked_at + INTERVAL '400 days'
    )
);

CREATE INDEX identity_links_customer_idx
    ON analytics.identity_links (
        merchant_account_id,
        store_id,
        customer_id,
        linked_at,
        id
    );

CREATE INDEX identity_links_retention_idx
    ON analytics.identity_links (merchant_account_id, retention_expires_at, id);

CREATE TABLE analytics.behavior_events (
    id                            UUID                         NOT NULL PRIMARY KEY,
    event_id                      UUID                         NOT NULL,
    merchant_account_id           UUID                         NOT NULL,
    store_id                      UUID                         NOT NULL,
    sales_channel_id              UUID                         NOT NULL,
    event_name                    analytics.browser_event_name NOT NULL,
    schema_version                SMALLINT                     NOT NULL,
    source                        analytics.event_source       NOT NULL,
    anonymous_id                  UUID                         NOT NULL,
    session_id                    UUID                         NOT NULL,
    analytics_storage_consent     BOOLEAN                      NOT NULL,
    advertising_storage_consent   BOOLEAN                      NOT NULL,
    advertising_export_eligible   BOOLEAN                      NOT NULL,
    consent_policy_version        TEXT                         NOT NULL,
    collection_policy_version     TEXT                         NOT NULL,
    properties                    JSONB                        NOT NULL,
    landing_path                  TEXT,
    referrer_domain               TEXT,
    campaign_source               TEXT,
    campaign_medium               TEXT,
    campaign_name                 TEXT,
    cart_id                       UUID,
    checkout_id                   UUID,
    occurred_at                   TIMESTAMPTZ                  NOT NULL,
    received_at                   TIMESTAMPTZ                  NOT NULL,
    retention_expires_at          TIMESTAMPTZ                  NOT NULL,
    created_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    UNIQUE (merchant_account_id, store_id, event_id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    CONSTRAINT behavior_events_schema_version_check CHECK (schema_version = 1),
    CONSTRAINT behavior_events_source_check CHECK (source = 'browser'),
    CONSTRAINT behavior_events_identity_check CHECK (
        event_id <> '00000000-0000-0000-0000-000000000000'::UUID
        AND anonymous_id <> '00000000-0000-0000-0000-000000000000'::UUID
        AND session_id <> '00000000-0000-0000-0000-000000000000'::UUID
    ),
    CONSTRAINT behavior_events_storage_consent_check CHECK (analytics_storage_consent),
    CONSTRAINT behavior_events_consent_policy_version_check CHECK (
        consent_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT behavior_events_collection_policy_version_check CHECK (
        collection_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT behavior_events_properties_check CHECK (
        jsonb_typeof(properties) = 'object' AND octet_length(properties::TEXT) <= 4096
    ),
    CONSTRAINT behavior_events_attribution_shape_check CHECK (
        (
            event_name = 'page_viewed'
            AND landing_path IS NOT NULL
            AND length(landing_path) BETWEEN 1 AND 1024
            AND (referrer_domain IS NULL OR length(referrer_domain) BETWEEN 1 AND 253)
            AND (campaign_source IS NULL OR length(campaign_source) BETWEEN 1 AND 100)
            AND (campaign_medium IS NULL OR length(campaign_medium) BETWEEN 1 AND 100)
            AND (campaign_name IS NULL OR length(campaign_name) BETWEEN 1 AND 200)
            AND (campaign_source IS NOT NULL
                OR (campaign_medium IS NULL AND campaign_name IS NULL))
        )
        OR (
            event_name <> 'page_viewed'
            AND landing_path IS NULL AND referrer_domain IS NULL
            AND campaign_source IS NULL AND campaign_medium IS NULL
            AND campaign_name IS NULL
        )
    ),
    CONSTRAINT behavior_events_commerce_reference_shape_check CHECK (
        (
            event_name IN ('cart_line_added', 'checkout_started')
            AND cart_id IS NOT NULL
            AND (event_name = 'checkout_started' OR checkout_id IS NULL)
        )
        OR (
            event_name NOT IN ('cart_line_added', 'checkout_started')
            AND cart_id IS NULL AND checkout_id IS NULL
        )
    ),
    CONSTRAINT behavior_events_export_eligibility_check CHECK (
        NOT advertising_export_eligible OR advertising_storage_consent
    ),
    CONSTRAINT behavior_events_timestamp_skew_check CHECK (
        occurred_at >= received_at - INTERVAL '24 hours'
        AND occurred_at <= received_at + INTERVAL '5 minutes'
    ),
    CONSTRAINT behavior_events_retention_check CHECK (
        retention_expires_at > received_at
        AND retention_expires_at <= received_at + INTERVAL '400 days'
    )
);

CREATE INDEX behavior_events_session_time_idx
    ON analytics.behavior_events (
        merchant_account_id,
        store_id,
        sales_channel_id,
        session_id,
        occurred_at,
        event_id
    );

CREATE INDEX behavior_events_retention_idx
    ON analytics.behavior_events (
        merchant_account_id,
        retention_expires_at,
        id
    );

CREATE INDEX behavior_events_attribution_touch_idx
    ON analytics.behavior_events (
        merchant_account_id,
        store_id,
        sales_channel_id,
        anonymous_id,
        session_id,
        occurred_at,
        id
    ) WHERE event_name = 'page_viewed';

CREATE INDEX behavior_events_checkout_attribution_idx
    ON analytics.behavior_events (
        merchant_account_id,
        store_id,
        sales_channel_id,
        checkout_id,
        cart_id,
        occurred_at DESC,
        id DESC
    ) WHERE event_name = 'checkout_started';

CREATE TABLE analytics.behavior_event_processing (
    id                    UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id   UUID                     NOT NULL,
    store_id              UUID                     NOT NULL,
    processing_status     integration.queue_status NOT NULL DEFAULT 'pending',
    attempts              INTEGER                  NOT NULL DEFAULT 0,
    available_at          TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by             UUID,
    locked_at             TIMESTAMPTZ,
    processed_at          TIMESTAMPTZ,
    last_error            TEXT,
    created_at            TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, id)
        REFERENCES analytics.behavior_events(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT behavior_event_processing_attempts_check CHECK (attempts BETWEEN 0 AND 31),
    CONSTRAINT behavior_event_processing_lease_shape_check CHECK (
        (processing_status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (processing_status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    ),
    CONSTRAINT behavior_event_processing_completion_shape_check CHECK (
        (processing_status = 'processed' AND processed_at IS NOT NULL)
        OR (processing_status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT behavior_event_processing_error_length_check CHECK (
        last_error IS NULL OR length(last_error) BETWEEN 1 AND 2000
    )
);

CREATE INDEX behavior_event_processing_claim_idx
    ON analytics.behavior_event_processing (processing_status, available_at, created_at, id)
    WHERE processing_status IN ('pending', 'processing');

CREATE TABLE analytics.sessions (
    id                               UUID        NOT NULL PRIMARY KEY,
    merchant_account_id              UUID        NOT NULL,
    store_id                         UUID        NOT NULL,
    sales_channel_id                 UUID        NOT NULL,
    anonymous_id                     UUID        NOT NULL,
    client_session_id                UUID        NOT NULL,
    started_at                       TIMESTAMPTZ NOT NULL,
    last_event_at                    TIMESTAMPTZ NOT NULL,
    event_count                      BIGINT      NOT NULL,
    page_view_count                  BIGINT      NOT NULL,
    product_view_count               BIGINT      NOT NULL,
    search_count                     BIGINT      NOT NULL,
    cart_line_added_count            BIGINT      NOT NULL,
    checkout_started_count           BIGINT      NOT NULL,
    active_engagement_milliseconds   BIGINT      NOT NULL,
    retention_expires_at             TIMESTAMPTZ NOT NULL,
    created_at                       TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                       TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    CONSTRAINT sessions_window_check CHECK (last_event_at >= started_at),
    CONSTRAINT sessions_counts_check CHECK (
        event_count > 0
        AND page_view_count >= 0
        AND product_view_count >= 0
        AND search_count >= 0
        AND cart_line_added_count >= 0
        AND checkout_started_count >= 0
        AND page_view_count + product_view_count + search_count
            + cart_line_added_count + checkout_started_count <= event_count
    ),
    CONSTRAINT sessions_engagement_check CHECK (
        active_engagement_milliseconds BETWEEN 0 AND 14400000
    ),
    CONSTRAINT sessions_retention_check CHECK (retention_expires_at > last_event_at)
);

CREATE INDEX sessions_identity_time_idx
    ON analytics.sessions (
        merchant_account_id,
        store_id,
        sales_channel_id,
        anonymous_id,
        client_session_id,
        last_event_at DESC,
        id
    );

CREATE INDEX sessions_retention_idx
    ON analytics.sessions (merchant_account_id, retention_expires_at, id);

CREATE TABLE analytics.erasure_requests (
    id                       UUID                      NOT NULL PRIMARY KEY,
    merchant_account_id      UUID                      NOT NULL,
    store_id                 UUID                      NOT NULL,
    anonymous_id             UUID,
    customer_id              UUID,
    status                   analytics.erasure_status NOT NULL DEFAULT 'pending',
    requested_by             UUID                      NOT NULL,
    behavior_events_deleted  BIGINT                    NOT NULL DEFAULT 0,
    attribution_results_deleted BIGINT                 NOT NULL DEFAULT 0,
    sessions_deleted         BIGINT                    NOT NULL DEFAULT 0,
    identity_links_deleted   BIGINT                    NOT NULL DEFAULT 0,
    requested_at             TIMESTAMPTZ               NOT NULL,
    completed_at             TIMESTAMPTZ,
    created_at               TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (requested_by) REFERENCES identity.users(id),
    CONSTRAINT erasure_requests_selector_check CHECK (
        (anonymous_id IS NOT NULL)::INTEGER + (customer_id IS NOT NULL)::INTEGER = 1
    ),
    CONSTRAINT erasure_requests_anonymous_id_check CHECK (
        anonymous_id IS NULL
        OR anonymous_id <> '00000000-0000-0000-0000-000000000000'::UUID
    ),
    CONSTRAINT erasure_requests_counts_check CHECK (
        behavior_events_deleted >= 0
        AND attribution_results_deleted >= 0
        AND sessions_deleted >= 0
        AND identity_links_deleted >= 0
    ),
    CONSTRAINT erasure_requests_completion_check CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status = 'pending' AND completed_at IS NULL)
    )
);

CREATE INDEX erasure_requests_pending_idx
    ON analytics.erasure_requests (status, requested_at, id)
    WHERE status = 'pending';

CREATE TABLE integration.webhook_inbox (
    id                         UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id        UUID                     NOT NULL,
    store_id                   UUID                     NOT NULL,
    provider                   TEXT                     NOT NULL,
    provider_event_id          TEXT                     NOT NULL,
    event_type                 TEXT                     NOT NULL,
    external_account_reference TEXT                     NOT NULL,
    payload                    JSONB                    NOT NULL,
    status                     integration.queue_status NOT NULL DEFAULT 'pending',
    attempts                   INTEGER                  NOT NULL DEFAULT 0,
    available_at               TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by                  UUID,
    locked_at                  TIMESTAMPTZ,
    processed_at               TIMESTAMPTZ,
    last_error                 TEXT,
    verified_at                TIMESTAMPTZ              NOT NULL,
    created_at                 TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider, provider_event_id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
    CONSTRAINT webhook_inbox_attempts_nonnegative_check CHECK (attempts >= 0),
    CONSTRAINT webhook_inbox_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT webhook_inbox_processed_shape_check CHECK (
        (status = 'processed' AND processed_at IS NOT NULL)
        OR (status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT webhook_inbox_lease_shape_check CHECK (
        (status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    )
);

CREATE INDEX webhook_inbox_claim_idx
    ON integration.webhook_inbox (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE TABLE integration.event_consumer_registry (
    event_type      TEXT PRIMARY KEY,
    consumer_owner  TEXT,
    description     TEXT NOT NULL,

    CONSTRAINT event_consumer_registry_event_type_check CHECK (
        event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT event_consumer_registry_owner_check CHECK (
        consumer_owner IS NULL
        OR consumer_owner ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT event_consumer_registry_description_check CHECK (
        length(trim(description)) BETWEEN 1 AND 255
    )
);

INSERT INTO integration.event_consumer_registry (event_type, consumer_owner, description)
VALUES
    ('payment.create_requested', 'payments.provider_dispatch',
     'Dispatches a Payment Attempt command to its configured provider'),
    ('refund.create_requested', 'payments.provider_dispatch',
     'Dispatches a Refund command to its configured provider'),
    ('search.product.changed', 'search.product_indexer',
     'Refreshes the Store-isolated Product search document'),
    ('fulfillment.shipped', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('fulfillment.delivered', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('fulfillment.cancelled', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('return.completed', 'fulfillment.operations',
     'Coordinates the immutable Return refund'),
    ('analytics.order.created', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Order creation fact'),
    ('analytics.payment.captured', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Payment capture fact'),
    ('analytics.refund.succeeded', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Refund success fact'),
    ('analytics.fulfillment.shipped', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Fulfillment shipment fact'),
    ('analytics.return.completed', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Return completion fact');

CREATE TABLE integration.outbox_events (
    id                   UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                     NOT NULL,
    store_id             UUID                     NOT NULL,
    aggregate_type       TEXT                     NOT NULL,
    aggregate_id         UUID                     NOT NULL,
    event_type           TEXT                     NOT NULL,
    payload              JSONB                    NOT NULL,
    status               integration.queue_status NOT NULL DEFAULT 'pending',
    attempts             INTEGER                  NOT NULL DEFAULT 0,
    available_at         TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by            UUID,
    locked_at            TIMESTAMPTZ,
    processed_at         TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (event_type)
        REFERENCES integration.event_consumer_registry(event_type),
    CONSTRAINT outbox_events_attempts_nonnegative_check CHECK (attempts >= 0),
    CONSTRAINT outbox_events_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT outbox_events_processed_shape_check CHECK (
        (status = 'processed' AND processed_at IS NOT NULL)
        OR (status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT outbox_events_lease_shape_check CHECK (
        (status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    )
);

CREATE INDEX outbox_events_claim_idx
    ON integration.outbox_events (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE TABLE analytics.commerce_facts (
    id                    UUID                          NOT NULL PRIMARY KEY,
    merchant_account_id   UUID                          NOT NULL,
    store_id              UUID                          NOT NULL,
    sales_channel_id      UUID                          NOT NULL,
    fact_name             analytics.commerce_fact_name NOT NULL,
    schema_version        SMALLINT                      NOT NULL,
    order_id              UUID                          NOT NULL,
    customer_id           UUID,
    payment_attempt_id    UUID,
    refund_id             UUID,
    fulfillment_id        UUID,
    return_id             UUID,
    amount_minor          BIGINT,
    currency              CHAR(3),
    occurred_at           TIMESTAMPTZ                   NOT NULL,
    ingested_at           TIMESTAMPTZ                   NOT NULL,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    FOREIGN KEY (id) REFERENCES integration.outbox_events(id),
    CONSTRAINT commerce_facts_schema_version_check CHECK (schema_version = 1),
    CONSTRAINT commerce_facts_currency_format_check CHECK (
        currency IS NULL OR currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT commerce_facts_amount_shape_check CHECK (
        (amount_minor IS NULL AND currency IS NULL)
        OR (amount_minor >= 0 AND currency IS NOT NULL)
    ),
    CONSTRAINT commerce_facts_reference_shape_check CHECK (
        (fact_name = 'order_created'
            AND payment_attempt_id IS NULL AND refund_id IS NULL
            AND fulfillment_id IS NULL AND return_id IS NULL
            AND amount_minor IS NOT NULL)
        OR (fact_name = 'payment_captured'
            AND payment_attempt_id IS NOT NULL AND refund_id IS NULL
            AND fulfillment_id IS NULL AND return_id IS NULL
            AND amount_minor IS NOT NULL)
        OR (fact_name = 'refund_succeeded'
            AND payment_attempt_id IS NOT NULL AND refund_id IS NOT NULL
            AND fulfillment_id IS NULL AND return_id IS NULL
            AND amount_minor IS NOT NULL)
        OR (fact_name = 'fulfillment_shipped'
            AND payment_attempt_id IS NULL AND refund_id IS NULL
            AND fulfillment_id IS NOT NULL AND return_id IS NULL
            AND amount_minor IS NULL)
        OR (fact_name = 'return_completed'
            AND payment_attempt_id IS NULL AND refund_id IS NULL
            AND fulfillment_id IS NULL AND return_id IS NOT NULL
            AND amount_minor IS NULL)
    )
);

CREATE INDEX commerce_facts_store_time_idx
    ON analytics.commerce_facts (
        merchant_account_id, store_id, occurred_at DESC, id DESC
    );

CREATE INDEX commerce_facts_store_name_time_idx
    ON analytics.commerce_facts (
        merchant_account_id, store_id, fact_name, occurred_at DESC, id DESC
    );

CREATE TABLE analytics.attribution_jobs (
    commerce_fact_id     UUID                     NOT NULL,
    merchant_account_id  UUID                     NOT NULL,
    store_id             UUID                     NOT NULL,
    model_version        SMALLINT                 NOT NULL,
    processing_status    integration.queue_status NOT NULL DEFAULT 'pending',
    attempts             INTEGER                  NOT NULL DEFAULT 0,
    available_at         TIMESTAMPTZ              NOT NULL,
    locked_by            UUID,
    locked_at            TIMESTAMPTZ,
    processed_at         TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (merchant_account_id, store_id, commerce_fact_id, model_version),
    FOREIGN KEY (merchant_account_id, store_id, commerce_fact_id)
        REFERENCES analytics.commerce_facts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT attribution_jobs_model_version_check CHECK (model_version > 0),
    CONSTRAINT attribution_jobs_attempts_check CHECK (attempts BETWEEN 0 AND 31),
    CONSTRAINT attribution_jobs_lease_shape_check CHECK (
        (processing_status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (processing_status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    ),
    CONSTRAINT attribution_jobs_completion_shape_check CHECK (
        (processing_status = 'processed' AND processed_at IS NOT NULL)
        OR (processing_status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT attribution_jobs_error_length_check CHECK (
        last_error IS NULL OR length(last_error) BETWEEN 1 AND 2000
    )
);

CREATE INDEX attribution_jobs_claim_idx
    ON analytics.attribution_jobs (processing_status, available_at, created_at, commerce_fact_id)
    WHERE processing_status IN ('pending', 'processing');

CREATE TABLE analytics.attribution_results (
    id                            UUID                        NOT NULL PRIMARY KEY,
    merchant_account_id           UUID                        NOT NULL,
    store_id                      UUID                        NOT NULL,
    sales_channel_id              UUID                        NOT NULL,
    commerce_fact_id              UUID                        NOT NULL,
    order_id                      UUID                        NOT NULL,
    customer_id                   UUID,
    checkout_id                   UUID                        NOT NULL,
    cart_id                       UUID                        NOT NULL,
    attribution_model             analytics.attribution_model NOT NULL,
    model_version                 SMALLINT                    NOT NULL,
    is_direct                     BOOLEAN                     NOT NULL,
    touch_event_id                UUID,
    anonymous_id                  UUID,
    session_id                    UUID,
    landing_path                  TEXT,
    referrer_domain               TEXT,
    campaign_source               TEXT,
    campaign_medium               TEXT,
    campaign_name                 TEXT,
    advertising_storage_consent   BOOLEAN,
    consent_policy_version        TEXT,
    collection_policy_version     TEXT,
    advertising_export_eligible   BOOLEAN                     NOT NULL,
    touch_occurred_at             TIMESTAMPTZ,
    input_event_watermark         TIMESTAMPTZ,
    attributed_at                 TIMESTAMPTZ                 NOT NULL,

    UNIQUE (merchant_account_id, store_id, commerce_fact_id, attribution_model, model_version),
    FOREIGN KEY (merchant_account_id, store_id, commerce_fact_id)
        REFERENCES analytics.commerce_facts(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, touch_event_id)
        REFERENCES analytics.behavior_events(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id),
    CONSTRAINT attribution_results_model_version_check CHECK (model_version > 0),
    CONSTRAINT attribution_results_touch_shape_check CHECK (
        (
            is_direct
            AND touch_event_id IS NULL AND anonymous_id IS NULL AND session_id IS NULL
            AND landing_path IS NULL AND referrer_domain IS NULL
            AND campaign_source IS NULL AND campaign_medium IS NULL AND campaign_name IS NULL
            AND advertising_storage_consent IS NULL
            AND consent_policy_version IS NULL AND collection_policy_version IS NULL
            AND NOT advertising_export_eligible
            AND touch_occurred_at IS NULL
        )
        OR (
            NOT is_direct
            AND touch_event_id IS NOT NULL AND anonymous_id IS NOT NULL AND session_id IS NOT NULL
            AND landing_path IS NOT NULL
            AND advertising_storage_consent IS NOT NULL
            AND consent_policy_version IS NOT NULL AND collection_policy_version IS NOT NULL
            AND touch_occurred_at IS NOT NULL
        )
    ),
    CONSTRAINT attribution_results_export_eligibility_check CHECK (
        NOT advertising_export_eligible OR advertising_storage_consent
    )
);

CREATE INDEX attribution_results_order_idx
    ON analytics.attribution_results (
        merchant_account_id, store_id, order_id, model_version, attribution_model
    );

CREATE INDEX attribution_results_destination_idx
    ON analytics.attribution_results (
        merchant_account_id, store_id, attributed_at, id
    ) WHERE advertising_export_eligible;

CREATE TABLE analytics.daily_behavior_reports (
    merchant_account_id             UUID        NOT NULL,
    store_id                        UUID        NOT NULL,
    sales_channel_id                UUID        NOT NULL,
    report_date                     DATE        NOT NULL,
    sessions                        BIGINT      NOT NULL,
    events                          BIGINT      NOT NULL,
    page_views                      BIGINT      NOT NULL,
    product_views                   BIGINT      NOT NULL,
    searches                        BIGINT      NOT NULL,
    cart_line_additions             BIGINT      NOT NULL,
    checkouts_started               BIGINT      NOT NULL,
    active_engagement_milliseconds  BIGINT      NOT NULL,
    refreshed_at                    TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, sales_channel_id, report_date),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT daily_behavior_reports_counts_check CHECK (
        sessions >= 0 AND events >= 0 AND page_views >= 0 AND product_views >= 0
        AND searches >= 0 AND cart_line_additions >= 0 AND checkouts_started >= 0
        AND active_engagement_milliseconds >= 0
    )
);

CREATE INDEX daily_behavior_reports_store_date_idx
    ON analytics.daily_behavior_reports (
        merchant_account_id, store_id, report_date DESC, sales_channel_id
    );

CREATE TABLE analytics.daily_commerce_reports (
    merchant_account_id       UUID        NOT NULL,
    store_id                  UUID        NOT NULL,
    sales_channel_id          UUID        NOT NULL,
    report_date               DATE        NOT NULL,
    currency                  CHAR(3)     NOT NULL,
    orders_created            BIGINT      NOT NULL,
    order_amount_minor        BIGINT      NOT NULL,
    payments_captured         BIGINT      NOT NULL,
    captured_amount_minor     BIGINT      NOT NULL,
    refunds_succeeded         BIGINT      NOT NULL,
    refunded_amount_minor     BIGINT      NOT NULL,
    fulfillments_shipped      BIGINT      NOT NULL,
    returns_completed         BIGINT      NOT NULL,
    refreshed_at              TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (merchant_account_id, store_id, sales_channel_id, report_date, currency),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT daily_commerce_reports_currency_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT daily_commerce_reports_counts_check CHECK (
        orders_created >= 0 AND order_amount_minor >= 0
        AND payments_captured >= 0 AND captured_amount_minor >= 0
        AND refunds_succeeded >= 0 AND refunded_amount_minor >= 0
        AND fulfillments_shipped >= 0 AND returns_completed >= 0
    )
);

CREATE INDEX daily_commerce_reports_store_date_idx
    ON analytics.daily_commerce_reports (
        merchant_account_id, store_id, report_date DESC, sales_channel_id, currency
    );

CREATE TABLE analytics.daily_attribution_reports (
    merchant_account_id  UUID                         NOT NULL,
    store_id             UUID                         NOT NULL,
    sales_channel_id     UUID                         NOT NULL,
    report_date          DATE                         NOT NULL,
    attribution_model    analytics.attribution_model  NOT NULL,
    model_version        SMALLINT                     NOT NULL,
    is_direct            BOOLEAN                      NOT NULL,
    campaign_source      TEXT                         NOT NULL DEFAULT '',
    campaign_medium      TEXT                         NOT NULL DEFAULT '',
    campaign_name        TEXT                         NOT NULL DEFAULT '',
    attributed_orders    BIGINT                       NOT NULL,
    attributed_amount_minor BIGINT                    NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    refreshed_at         TIMESTAMPTZ                  NOT NULL,

    PRIMARY KEY (
        merchant_account_id, store_id, sales_channel_id, report_date,
        attribution_model, model_version, is_direct,
        campaign_source, campaign_medium, campaign_name, currency
    ),
    FOREIGN KEY (merchant_account_id, store_id, sales_channel_id)
        REFERENCES merchant.sales_channels(merchant_account_id, store_id, id) ON DELETE CASCADE,
    CONSTRAINT daily_attribution_reports_model_version_check CHECK (model_version > 0),
    CONSTRAINT daily_attribution_reports_campaign_check CHECK (
        length(campaign_source) <= 100
        AND length(campaign_medium) <= 100
        AND length(campaign_name) <= 200
        AND (NOT is_direct OR (
            campaign_source = '' AND campaign_medium = '' AND campaign_name = ''
        ))
    ),
    CONSTRAINT daily_attribution_reports_currency_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT daily_attribution_reports_counts_check CHECK (
        attributed_orders >= 0 AND attributed_amount_minor >= 0
    )
);

CREATE INDEX daily_attribution_reports_store_date_idx
    ON analytics.daily_attribution_reports (
        merchant_account_id, store_id, report_date DESC,
        attribution_model, model_version, sales_channel_id
    );

CREATE TABLE sales.order_fulfillment_transitions (
    id                       UUID                           NOT NULL PRIMARY KEY,
    merchant_account_id      UUID                           NOT NULL,
    store_id                 UUID                           NOT NULL,
    order_id                 UUID                           NOT NULL,
    source_event_id          UUID                           NOT NULL UNIQUE,
    from_fulfillment_status  sales.order_fulfillment_status NOT NULL,
    to_fulfillment_status    sales.order_fulfillment_status NOT NULL,
    from_delivery_status     sales.order_delivery_status    NOT NULL,
    to_delivery_status       sales.order_delivery_status    NOT NULL,
    occurred_at              TIMESTAMPTZ                    NOT NULL,

    UNIQUE (merchant_account_id, store_id, order_id, id),
    FOREIGN KEY (merchant_account_id, store_id, order_id)
        REFERENCES sales.orders(merchant_account_id, store_id, id),
    FOREIGN KEY (source_event_id) REFERENCES integration.outbox_events(id)
);

CREATE INDEX order_fulfillment_transitions_order_idx
    ON sales.order_fulfillment_transitions (
        merchant_account_id,
        store_id,
        order_id,
        occurred_at,
        id
    );

CREATE TABLE search.product_documents (
    merchant_account_id UUID        NOT NULL,
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    document            TSVECTOR    NOT NULL,
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (merchant_account_id, store_id, product_id),
    FOREIGN KEY (merchant_account_id, store_id, product_id)
        REFERENCES catalog.products(merchant_account_id, store_id, id) ON DELETE CASCADE
);

CREATE INDEX product_documents_search_idx
    ON search.product_documents USING GIN (document);

CREATE FUNCTION search.refresh_product_document(UUID, UUID, UUID)
RETURNS VOID LANGUAGE SQL SECURITY DEFINER SET search_path = pg_catalog AS $$
    INSERT INTO search.product_documents (
        merchant_account_id, store_id, product_id, document, indexed_at
    )
    SELECT product.merchant_account_id, product.store_id, product.id,
           to_tsvector('simple', concat_ws(
               ' ', product.handle::text, product.title, product.description,
               string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
           )), CURRENT_TIMESTAMP
      FROM catalog.products AS product
      LEFT JOIN catalog.product_variants AS variant
        ON variant.merchant_account_id = product.merchant_account_id
       AND variant.store_id = product.store_id AND variant.product_id = product.id
     WHERE product.merchant_account_id = $1 AND product.store_id = $2 AND product.id = $3
     GROUP BY product.merchant_account_id, product.store_id, product.id
    ON CONFLICT (merchant_account_id, store_id, product_id) DO UPDATE
        SET document = EXCLUDED.document, indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION search.capture_product_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO integration.outbox_events (
        id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload
    ) VALUES (
        uuidv7(), NEW.merchant_account_id, NEW.store_id, 'product', NEW.id,
        'search.product.changed', jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION search.capture_variant_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    account_id UUID;
    owning_store_id UUID;
    changed_product_id UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        account_id := OLD.merchant_account_id;
        owning_store_id := OLD.store_id;
        changed_product_id := OLD.product_id;
    ELSE
        account_id := NEW.merchant_account_id;
        owning_store_id := NEW.store_id;
        changed_product_id := NEW.product_id;
    END IF;
    IF EXISTS (
        SELECT 1 FROM merchant.stores
         WHERE merchant_account_id = account_id AND id = owning_store_id
    ) THEN
        INSERT INTO integration.outbox_events (
            id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload
        ) VALUES (
            uuidv7(), account_id, owning_store_id, 'product', changed_product_id,
            'search.product.changed', jsonb_build_object('product_id', changed_product_id)
        );
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER products_search_change
AFTER INSERT OR UPDATE OF handle, title, description ON catalog.products
FOR EACH ROW EXECUTE FUNCTION search.capture_product_change();
CREATE TRIGGER variants_search_change
AFTER INSERT OR UPDATE OF title, sku OR DELETE ON catalog.product_variants
FOR EACH ROW EXECUTE FUNCTION search.capture_variant_change();

CREATE FUNCTION search.rebuild_store_products(UUID, UUID)
RETURNS BIGINT LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE product_id UUID; rebuilt BIGINT := 0;
BEGIN
    DELETE FROM search.product_documents WHERE merchant_account_id = $1 AND store_id = $2;
    FOR product_id IN SELECT id FROM catalog.products
        WHERE merchant_account_id = $1 AND store_id = $2
    LOOP
        PERFORM search.refresh_product_document($1, $2, product_id);
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
        SELECT outbox.id, outbox.merchant_account_id, outbox.store_id, outbox.aggregate_id
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
            event.merchant_account_id, event.store_id, event.aggregate_id
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

CREATE FUNCTION integration.queue_metrics()
RETURNS TABLE (pending BIGINT, dead_letter BIGINT, oldest_pending_seconds DOUBLE PRECISION)
LANGUAGE SQL STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT count(*) FILTER (WHERE status = 'pending'),
           count(*) FILTER (WHERE status = 'dead_letter'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       min(created_at) FILTER (WHERE status = 'pending')
               ),
               0
           )
      FROM integration.outbox_events;
$$;

CREATE FUNCTION integration.event_consumer_backlog()
RETURNS TABLE (
    event_type TEXT,
    consumer_owner TEXT,
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    processed BIGINT
)
LANGUAGE SQL STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT registry.event_type,
           registry.consumer_owner,
           count(event.id) FILTER (WHERE event.status = 'pending'),
           count(event.id) FILTER (WHERE event.status = 'processing'),
           count(event.id) FILTER (WHERE event.status = 'dead_letter'),
           count(event.id) FILTER (WHERE event.status = 'processed')
      FROM integration.event_consumer_registry AS registry
      LEFT JOIN integration.outbox_events AS event
        ON event.event_type = registry.event_type
     GROUP BY registry.event_type, registry.consumer_owner
     ORDER BY registry.event_type;
$$;

CREATE FUNCTION payments.provider_readiness_metrics()
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
      FROM payments.provider_accounts AS account;
$$;

CREATE FUNCTION notification.email_delivery_metrics()
RETURNS TABLE (
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    suppressed BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (WHERE delivery.delivery_status = 'pending'),
           count(*) FILTER (WHERE delivery.delivery_status = 'processing'),
           count(*) FILTER (WHERE delivery.delivery_status = 'dead_letter'),
           count(*) FILTER (WHERE delivery.delivery_status = 'suppressed'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(delivery.created_at)
                            FILTER (WHERE delivery.delivery_status = 'pending'))
               ),
               0
           )::DOUBLE PRECISION
      FROM notification.email_deliveries AS delivery;
$$;

CREATE FUNCTION fulfillment.shipping_tracking_metrics()
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
      FROM fulfillment.shipping_labels AS label
     WHERE label.purchase_state = 'purchased'
       AND label.provider_tracker_reference IS NOT NULL;
$$;

CREATE FUNCTION fulfillment.shipping_cancellation_metrics()
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
      FROM fulfillment.shipping_labels AS label
     WHERE label.cancellation_status = 'submitted';
$$;

CREATE FUNCTION analytics.sessionization_metrics()
RETURNS TABLE (
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (WHERE job.processing_status = 'pending'),
           count(*) FILTER (WHERE job.processing_status = 'processing'),
           count(*) FILTER (WHERE job.processing_status = 'dead_letter'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(job.created_at)
                            FILTER (WHERE job.processing_status = 'pending'))
               ),
               0
           )::DOUBLE PRECISION
      FROM analytics.behavior_event_processing AS job;
$$;

CREATE FUNCTION analytics.attribution_metrics()
RETURNS TABLE (
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (WHERE job.processing_status = 'pending'),
           count(*) FILTER (WHERE job.processing_status = 'processing'),
           count(*) FILTER (WHERE job.processing_status = 'dead_letter'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(job.available_at)
                            FILTER (WHERE job.processing_status = 'pending'))
               ),
               0
           )::DOUBLE PRECISION
      FROM analytics.attribution_jobs AS job;
$$;

CREATE FUNCTION analytics.retention_metrics()
RETURNS TABLE (
    expired_behavior_events BIGINT,
    expired_sessions BIGINT,
    expired_identity_links BIGINT,
    oldest_expired_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT (SELECT count(*) FROM analytics.behavior_events AS event
             WHERE event.retention_expires_at <= CURRENT_TIMESTAMP),
           (SELECT count(*) FROM analytics.sessions AS session
             WHERE session.retention_expires_at <= CURRENT_TIMESTAMP),
           (SELECT count(*) FROM analytics.identity_links AS link
             WHERE link.retention_expires_at <= CURRENT_TIMESTAMP),
           greatest(
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(event.retention_expires_at))
                     FROM analytics.behavior_events AS event
                    WHERE event.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0),
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(session.retention_expires_at))
                     FROM analytics.sessions AS session
                    WHERE session.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0),
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(link.retention_expires_at))
                     FROM analytics.identity_links AS link
                    WHERE link.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0)
           )::DOUBLE PRECISION;
$$;

CREATE FUNCTION analytics.apply_store_retention_policy(
    requested_merchant_account_id UUID,
    requested_store_id UUID,
    retention_days INTEGER
)
RETURNS TABLE (
    behavior_events_updated BIGINT,
    sessions_updated BIGINT,
    identity_links_updated BIGINT
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH updated_events AS (
        UPDATE analytics.behavior_events AS event
           SET retention_expires_at = least(
                   event.retention_expires_at,
                   event.received_at + make_interval(days => retention_days)
               )
         WHERE event.merchant_account_id = requested_merchant_account_id
           AND event.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_merchant_account_id =
               nullif(current_setting('app.merchant_account_id', true), '')::uuid
        RETURNING 1
    ), updated_sessions AS (
        UPDATE analytics.sessions AS session
           SET retention_expires_at = least(
                   session.retention_expires_at,
                   session.last_event_at + make_interval(days => retention_days)
               ),
               updated_at = CURRENT_TIMESTAMP
         WHERE session.merchant_account_id = requested_merchant_account_id
           AND session.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_merchant_account_id =
               nullif(current_setting('app.merchant_account_id', true), '')::uuid
        RETURNING 1
    ), updated_links AS (
        UPDATE analytics.identity_links AS link
           SET retention_expires_at = least(
                   link.retention_expires_at,
                   link.linked_at + make_interval(days => retention_days)
               )
         WHERE link.merchant_account_id = requested_merchant_account_id
           AND link.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_merchant_account_id =
               nullif(current_setting('app.merchant_account_id', true), '')::uuid
        RETURNING 1
    )
    SELECT (SELECT count(*) FROM updated_events),
           (SELECT count(*) FROM updated_sessions),
           (SELECT count(*) FROM updated_links);
$$;

CREATE FUNCTION analytics.purge_expired_data(batch_size INTEGER, purged_at TIMESTAMPTZ)
RETURNS TABLE (
    behavior_events_deleted BIGINT,
    attribution_results_deleted BIGINT,
    sessions_deleted BIGINT,
    identity_links_deleted BIGINT
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired_sessions AS (
        SELECT session.id
          FROM analytics.sessions AS session
         WHERE session.retention_expires_at <= purged_at
         ORDER BY session.retention_expires_at, session.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), deleted_sessions AS (
        DELETE FROM analytics.sessions AS session
         USING expired_sessions
         WHERE session.id = expired_sessions.id
        RETURNING 1
    ), expired_events AS (
        SELECT event.id
          FROM analytics.behavior_events AS event
         WHERE event.retention_expires_at <= purged_at
         ORDER BY event.retention_expires_at, event.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), expired_attributions AS (
        SELECT count(*) AS deleted_count
          FROM analytics.attribution_results AS result
          INNER JOIN expired_events ON expired_events.id = result.touch_event_id
    ), deleted_events AS (
        DELETE FROM analytics.behavior_events AS event
         USING expired_events
         WHERE event.id = expired_events.id
        RETURNING 1
    ), expired_links AS (
        SELECT link.id
          FROM analytics.identity_links AS link
         WHERE link.retention_expires_at <= purged_at
         ORDER BY link.retention_expires_at, link.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), deleted_links AS (
        DELETE FROM analytics.identity_links AS link
         USING expired_links
         WHERE link.id = expired_links.id
        RETURNING 1
    )
    SELECT (SELECT count(*) FROM deleted_events),
           (SELECT deleted_count FROM expired_attributions),
           (SELECT count(*) FROM deleted_sessions),
           (SELECT count(*) FROM deleted_links);
$$;

CREATE FUNCTION analytics.process_erasure_requests(batch_size INTEGER, processed_at TIMESTAMPTZ)
RETURNS TABLE (
    requests_completed BIGINT,
    behavior_events_deleted BIGINT,
    attribution_results_deleted BIGINT,
    sessions_deleted BIGINT,
    identity_links_deleted BIGINT
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    request_row RECORD;
    target_anonymous_ids UUID[];
    request_behavior_events BIGINT;
    request_attribution_results BIGINT;
    request_sessions BIGINT;
    request_identity_links BIGINT;
    total_requests BIGINT := 0;
    total_behavior_events BIGINT := 0;
    total_attribution_results BIGINT := 0;
    total_sessions BIGINT := 0;
    total_identity_links BIGINT := 0;
BEGIN
    FOR request_row IN
        SELECT request.id,
               request.merchant_account_id,
               request.store_id,
               request.anonymous_id,
               request.customer_id
          FROM analytics.erasure_requests AS request
         WHERE request.status = 'pending'
         ORDER BY request.requested_at, request.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    LOOP
        SELECT coalesce(array_agg(target.anonymous_id), ARRAY[]::UUID[])
          INTO target_anonymous_ids
          FROM (
              SELECT request_row.anonymous_id AS anonymous_id
               WHERE request_row.anonymous_id IS NOT NULL
              UNION
              SELECT link.anonymous_id
                FROM analytics.identity_links AS link
               WHERE link.merchant_account_id = request_row.merchant_account_id
                 AND link.store_id = request_row.store_id
                 AND link.customer_id = request_row.customer_id
          ) AS target;

        SELECT count(*)
          INTO request_attribution_results
          FROM analytics.attribution_results AS result
         WHERE result.merchant_account_id = request_row.merchant_account_id
           AND result.store_id = request_row.store_id
           AND result.anonymous_id = ANY(target_anonymous_ids);

        DELETE FROM analytics.behavior_events AS event
         WHERE event.merchant_account_id = request_row.merchant_account_id
           AND event.store_id = request_row.store_id
           AND event.anonymous_id = ANY(target_anonymous_ids);
        GET DIAGNOSTICS request_behavior_events = ROW_COUNT;

        DELETE FROM analytics.sessions AS session
         WHERE session.merchant_account_id = request_row.merchant_account_id
           AND session.store_id = request_row.store_id
           AND session.anonymous_id = ANY(target_anonymous_ids);
        GET DIAGNOSTICS request_sessions = ROW_COUNT;

        DELETE FROM analytics.identity_links AS link
         WHERE link.merchant_account_id = request_row.merchant_account_id
           AND link.store_id = request_row.store_id
           AND (
               link.anonymous_id = ANY(target_anonymous_ids)
               OR (
                   request_row.customer_id IS NOT NULL
                   AND link.customer_id = request_row.customer_id
               )
           );
        GET DIAGNOSTICS request_identity_links = ROW_COUNT;

        UPDATE analytics.erasure_requests AS request
           SET status = 'completed',
               behavior_events_deleted = request_behavior_events,
               attribution_results_deleted = request_attribution_results,
               sessions_deleted = request_sessions,
               identity_links_deleted = request_identity_links,
               completed_at = processed_at,
               updated_at = processed_at
         WHERE request.id = request_row.id
           AND request.status = 'pending';

        total_requests := total_requests + 1;
        total_behavior_events := total_behavior_events + request_behavior_events;
        total_attribution_results := total_attribution_results + request_attribution_results;
        total_sessions := total_sessions + request_sessions;
        total_identity_links := total_identity_links + request_identity_links;
    END LOOP;

    RETURN QUERY SELECT total_requests,
                        total_behavior_events,
                        total_attribution_results,
                        total_sessions,
                        total_identity_links;
END;
$$;

CREATE FUNCTION analytics.erasure_metrics()
RETURNS TABLE (
    pending BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (WHERE request.status = 'pending'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       min(request.requested_at) FILTER (WHERE request.status = 'pending')
               ),
               0
           )::DOUBLE PRECISION
      FROM analytics.erasure_requests AS request;
$$;

ALTER TABLE merchant.merchant_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.idempotency_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.merchant_account_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.stores ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.store_currencies ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.store_domains ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.sales_channels ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.api_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.api_key_scopes ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.products ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.product_options ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.product_option_values ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.product_variants ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.variant_selected_options ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.product_publications ENABLE ROW LEVEL SECURITY;
ALTER TABLE pricing.price_lists ENABLE ROW LEVEL SECURITY;
ALTER TABLE pricing.prices ENABLE ROW LEVEL SECURITY;
ALTER TABLE pricing.tax_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE pricing.promotions ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory.inventory_locations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory.stock_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory.inventory_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory.inventory_reservation_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory.stock_ledger_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.customers ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.customer_addresses ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.customer_shopper_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.carts ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.cart_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkouts ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_contacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_addresses ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_tax_calculations ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_promotion_calculations ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.checkout_shipping_selections ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_contacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_addresses ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_tax_calculations ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_promotion_calculations ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_shipping_selections ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_transitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales.order_fulfillment_transitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE payments.provider_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE payments.payment_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE payments.refunds ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.webhook_inbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.outbox_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE search.product_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.fulfillments ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_services ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_provider_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_service_regions ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.fulfillment_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_quote_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_rate_quotes ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.shipping_labels ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.returns ENABLE ROW LEVEL SECURITY;
ALTER TABLE fulfillment.return_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification.email_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification.email_suppressions ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification.webhook_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.behavior_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.store_policy_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.identity_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.behavior_event_processing ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.erasure_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.commerce_facts ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.attribution_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.attribution_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.daily_behavior_reports ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.daily_commerce_reports ENABLE ROW LEVEL SECURITY;
ALTER TABLE analytics.daily_attribution_reports ENABLE ROW LEVEL SECURITY;

CREATE POLICY idempotency_scope_isolation ON integration.idempotency_records
    USING (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'merchant_account' AND scope_id =
            nullif(current_setting('app.merchant_account_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    )
    WITH CHECK (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'merchant_account' AND scope_id =
            nullif(current_setting('app.merchant_account_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    );

CREATE POLICY merchant_account_isolation ON merchant.merchant_accounts
    USING (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid);

CREATE POLICY merchant_account_directory ON merchant.merchant_accounts
    FOR SELECT
    USING (
        EXISTS (
            SELECT 1
            FROM merchant.merchant_account_memberships AS membership
            WHERE membership.merchant_account_id = merchant_accounts.id
              AND membership.user_id =
                    nullif(current_setting('app.user_id', true), '')::uuid
        )
    );

CREATE POLICY merchant_account_isolation ON merchant.merchant_account_memberships
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_membership_directory ON merchant.merchant_account_memberships
    FOR SELECT
    USING (
        user_id = nullif(current_setting('app.user_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.stores
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.store_currencies
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.store_domains
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.sales_channels
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.api_keys
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.api_key_scopes
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.products
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.product_options
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.product_option_values
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.product_variants
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.variant_selected_options
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.product_publications
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON pricing.price_lists
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON pricing.prices
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON pricing.tax_rules
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON pricing.promotions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON inventory.inventory_locations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON inventory.stock_items
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON inventory.inventory_reservations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON inventory.inventory_reservation_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON inventory.stock_ledger_entries
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.carts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.customers
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.customer_addresses
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.customer_shopper_links
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.cart_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkouts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_contacts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_addresses
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_tax_calculations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_promotion_calculations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.checkout_shipping_selections
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.orders
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_contacts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_addresses
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_tax_calculations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_promotion_calculations
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_shipping_selections
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_transitions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON sales.order_fulfillment_transitions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON payments.provider_accounts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON payments.payment_attempts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON payments.refunds
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON integration.webhook_inbox
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON integration.outbox_events
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON search.product_documents
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.fulfillments
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_services
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_provider_accounts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_service_regions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.fulfillment_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_quote_requests
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_rate_quotes
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.shipping_labels
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.returns
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON fulfillment.return_lines
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON notification.email_deliveries
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON notification.email_suppressions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON notification.webhook_events
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.behavior_events
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.store_policy_versions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.identity_links
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.behavior_event_processing
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.sessions
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.erasure_requests
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.commerce_facts
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.attribution_jobs
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.attribution_results
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.daily_behavior_reports
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.daily_commerce_reports
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON analytics.daily_attribution_reports
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE FUNCTION payments.resolve_provider_account(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
)
RETURNS TABLE (
    provider_account_id UUID,
    merchant_account_id UUID,
    store_id            UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.merchant_account_id, account.store_id
    FROM payments.provider_accounts AS account
    WHERE account.provider = requested_provider
      AND account.external_account_reference = requested_external_account_reference;
$$;

CREATE FUNCTION payments.resolve_provider_webhook_secret_references(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
)
RETURNS TABLE (secret_reference TEXT)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT candidate.secret_reference
    FROM payments.provider_accounts AS account
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
      AND account.external_account_reference = requested_external_account_reference
      AND candidate.secret_reference IS NOT NULL
    ORDER BY candidate.priority;
$$;

CREATE FUNCTION payments.claim_provider_readiness_checks(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id UUID,
    merchant_account_id UUID,
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
        UPDATE payments.provider_accounts AS account
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
        FROM payments.provider_accounts AS account
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
    UPDATE payments.provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.merchant_account_id, account.store_id, account.provider,
              account.external_account_reference, account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

CREATE FUNCTION payments.finish_provider_readiness_check(
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
    UPDATE payments.provider_accounts AS account
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

CREATE FUNCTION integration.claim_outbox_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.outbox_events AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
          FROM integration.event_consumer_registry AS registry
         WHERE registry.event_type = event.event_type
           AND registry.consumer_owner = 'payments.provider_dispatch'
           AND event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.outbox_events AS event
        INNER JOIN integration.event_consumer_registry AS registry
          ON registry.event_type = event.event_type
         AND registry.consumer_owner = 'payments.provider_dispatch'
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE OF event SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.outbox_events AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.merchant_account_id, event.store_id,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.claim_fulfillment_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.outbox_events AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
          FROM integration.event_consumer_registry AS registry
         WHERE registry.event_type = event.event_type
           AND registry.consumer_owner = 'fulfillment.operations'
           AND event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.outbox_events AS event
        INNER JOIN integration.event_consumer_registry AS registry
          ON registry.event_type = event.event_type
         AND registry.consumer_owner = 'fulfillment.operations'
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE OF event SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.outbox_events AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.merchant_account_id, event.store_id,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION analytics.claim_commerce_fact_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER,
    occurred_at TIMESTAMPTZ
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.outbox_events AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
          FROM integration.event_consumer_registry AS registry
         WHERE registry.event_type = event.event_type
           AND registry.consumer_owner = 'analytics.commerce_fact_ingestor'
           AND event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
          FROM integration.outbox_events AS event
          INNER JOIN integration.event_consumer_registry AS registry
            ON registry.event_type = event.event_type
           AND registry.consumer_owner = 'analytics.commerce_fact_ingestor'
         WHERE (
                 (event.status = 'pending' AND event.available_at <= claimed_at)
                 OR (event.status = 'processing' AND event.locked_at <= stale_before)
               )
           AND event.attempts < 8
         ORDER BY event.available_at, event.created_at, event.id
         FOR UPDATE OF event SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.outbox_events AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.merchant_account_id, event.store_id,
              event.event_type, event.payload, event.attempts, event.created_at;
$$;

CREATE FUNCTION analytics.claim_attribution_jobs(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    commerce_fact_id UUID,
    merchant_account_id UUID,
    store_id UUID,
    model_version SMALLINT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE analytics.attribution_jobs AS job
           SET processing_status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(job.last_error, 'worker lease expired after final attempt'),
               updated_at = claimed_at
         WHERE job.processing_status = 'processing'
           AND job.locked_at <= stale_before
           AND job.attempts >= 8
        RETURNING job.commerce_fact_id
    ), claimable AS (
        SELECT job.merchant_account_id,
               job.store_id,
               job.commerce_fact_id,
               job.model_version
          FROM analytics.attribution_jobs AS job
         WHERE (
                 (job.processing_status = 'pending' AND job.available_at <= claimed_at)
                 OR (job.processing_status = 'processing' AND job.locked_at <= stale_before)
               )
           AND job.attempts < 8
         ORDER BY job.available_at, job.created_at, job.commerce_fact_id
         FOR UPDATE OF job SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE analytics.attribution_jobs AS job
       SET processing_status = 'processing',
           attempts = job.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at,
           updated_at = claimed_at
      FROM claimable
     WHERE job.merchant_account_id = claimable.merchant_account_id
       AND job.store_id = claimable.store_id
       AND job.commerce_fact_id = claimable.commerce_fact_id
       AND job.model_version = claimable.model_version
    RETURNING job.commerce_fact_id, job.merchant_account_id, job.store_id,
              job.model_version, job.attempts;
$$;

CREATE FUNCTION analytics.rebuild_store_attribution(
    requested_merchant_account_id UUID,
    requested_store_id UUID,
    requested_model_version SMALLINT,
    requested_at TIMESTAMPTZ
)
RETURNS BIGINT
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE rebuilt BIGINT;
BEGIN
    IF requested_merchant_account_id <>
       nullif(current_setting('app.merchant_account_id', true), '')::uuid THEN
        RETURN 0;
    END IF;
    IF requested_model_version <= 0 THEN
        RETURN 0;
    END IF;

    DELETE FROM analytics.attribution_results AS result
     WHERE result.merchant_account_id = requested_merchant_account_id
       AND result.store_id = requested_store_id
       AND result.model_version = requested_model_version;

    UPDATE analytics.attribution_jobs AS job
       SET processing_status = 'pending',
           attempts = 0,
           available_at = requested_at,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = NULL,
           last_error = NULL,
           updated_at = requested_at
     WHERE job.merchant_account_id = requested_merchant_account_id
       AND job.store_id = requested_store_id
       AND job.model_version = requested_model_version;
    GET DIAGNOSTICS rebuilt = ROW_COUNT;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION sales.claim_expired_checkouts(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
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
        FROM sales.checkouts AS checkout
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
    UPDATE sales.checkouts AS checkout
       SET expiry_locked_by = worker_id,
           expiry_locked_at = claimed_at
      FROM claimable
     WHERE checkout.id = claimable.id
    RETURNING checkout.id, checkout.merchant_account_id, checkout.store_id,
              checkout.inventory_reservation_id;
$$;

CREATE FUNCTION integration.claim_webhook_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
    store_id UUID,
    provider TEXT,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.webhook_inbox AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
         WHERE event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.webhook_inbox AS event
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.webhook_inbox AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.merchant_account_id, event.store_id, event.provider,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.finish_outbox_event(
    event_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    failure TEXT,
    max_attempts INTEGER,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE integration.outbox_events AS event
       SET status = CASE
               WHEN succeeded THEN 'processed'::integration.queue_status
               WHEN event.attempts >= greatest(max_attempts, 1)
                   THEN 'dead_letter'::integration.queue_status
               ELSE 'pending'::integration.queue_status
           END,
           available_at = CASE
               WHEN succeeded THEN event.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(event.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
           last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
     WHERE event.id = event_id AND event.status = 'processing' AND event.locked_by = worker_id
    RETURNING true;
$$;

CREATE FUNCTION integration.finish_webhook_event(
    event_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    failure TEXT,
    max_attempts INTEGER,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE integration.webhook_inbox AS event
       SET status = CASE
               WHEN succeeded THEN 'processed'::integration.queue_status
               WHEN event.attempts >= greatest(max_attempts, 1)
                   THEN 'dead_letter'::integration.queue_status
               ELSE 'pending'::integration.queue_status
           END,
           available_at = CASE
               WHEN succeeded THEN event.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(event.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
           last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
     WHERE event.id = event_id AND event.status = 'processing' AND event.locked_by = worker_id
    RETURNING true;
$$;

CREATE FUNCTION notification.claim_email_deliveries(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    merchant_account_id UUID,
    store_id UUID,
    recipient_email TEXT,
    template_key TEXT,
    template_version INTEGER,
    template_payload JSONB,
    provider TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH suppress AS (
        UPDATE notification.email_deliveries AS delivery
           SET delivery_status = 'suppressed',
               locked_by = NULL,
               locked_at = NULL,
               last_error = 'recipient is suppressed',
               updated_at = claimed_at
         WHERE delivery.delivery_status IN ('pending', 'processing')
           AND EXISTS (
               SELECT 1
                 FROM notification.email_suppressions AS suppression
                WHERE suppression.merchant_account_id = delivery.merchant_account_id
                  AND suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
        RETURNING delivery.id
    ), expired AS (
        UPDATE notification.email_deliveries AS delivery
           SET delivery_status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(delivery.last_error, 'worker lease expired after final attempt'),
               updated_at = claimed_at
         WHERE delivery.delivery_status = 'processing'
           AND delivery.locked_at <= stale_before
           AND delivery.attempts >= 8
        RETURNING delivery.id
    ), claimable AS (
        SELECT delivery.id
          FROM notification.email_deliveries AS delivery
         WHERE (
                 (delivery.delivery_status = 'pending' AND delivery.available_at <= claimed_at)
                 OR (delivery.delivery_status = 'processing' AND delivery.locked_at <= stale_before)
               )
           AND delivery.attempts < 8
           AND NOT EXISTS (
               SELECT 1
                 FROM notification.email_suppressions AS suppression
                WHERE suppression.merchant_account_id = delivery.merchant_account_id
                  AND suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
         ORDER BY delivery.available_at, delivery.created_at, delivery.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE notification.email_deliveries AS delivery
       SET delivery_status = 'processing',
           attempts = delivery.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at,
           updated_at = claimed_at
      FROM claimable
     WHERE delivery.id = claimable.id
    RETURNING delivery.id, delivery.merchant_account_id, delivery.store_id,
              delivery.recipient_email::text, delivery.template_key,
              delivery.template_version, delivery.template_payload,
              delivery.provider, delivery.attempts;
$$;

CREATE FUNCTION notification.finish_email_delivery(
    delivery_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    retryable BOOLEAN,
    provider_message_id TEXT,
    failure TEXT,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE notification.email_deliveries AS delivery
       SET delivery_status = CASE
               WHEN succeeded THEN 'sent'::notification.email_delivery_status
               WHEN NOT retryable THEN 'failed'::notification.email_delivery_status
               WHEN delivery.attempts >= 8 THEN 'dead_letter'::notification.email_delivery_status
               ELSE 'pending'::notification.email_delivery_status
           END,
           provider_message_id = CASE
               WHEN succeeded THEN $5 ELSE delivery.provider_message_id
           END,
           available_at = CASE
               WHEN succeeded OR NOT retryable THEN delivery.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(delivery.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           sent_at = CASE WHEN succeeded THEN finished_at ELSE delivery.sent_at END,
           last_error = CASE
               WHEN succeeded THEN NULL
               ELSE COALESCE(NULLIF(left(failure, 2000), ''), 'email delivery failed')
           END,
           updated_at = finished_at
     WHERE delivery.id = $1
       AND delivery.delivery_status = 'processing'
       AND delivery.locked_by = $2
    RETURNING true;
$$;

CREATE FUNCTION notification.record_resend_webhook(
    provider_event_id TEXT,
    provider_message_id TEXT,
    provider_event_type TEXT,
    payload JSONB,
    received_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    target notification.email_deliveries%ROWTYPE;
    webhook_id UUID;
    suppression_reason notification.email_suppression_reason;
BEGIN
    SELECT delivery.*
      INTO target
      FROM notification.email_deliveries AS delivery
     WHERE delivery.provider = 'resend'
       AND delivery.provider_message_id = record_resend_webhook.provider_message_id
     FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    webhook_id := uuidv7();
    INSERT INTO notification.webhook_events (
        id, merchant_account_id, store_id, delivery_id, provider, provider_event_id,
        provider_event_type, payload, received_at, processed_at
    ) VALUES (
        webhook_id, target.merchant_account_id, target.store_id, target.id, 'resend',
        record_resend_webhook.provider_event_id, record_resend_webhook.provider_event_type,
        payload, received_at, received_at
    ) ON CONFLICT ON CONSTRAINT webhook_events_provider_provider_event_id_key DO NOTHING;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    IF provider_event_type = 'email.sent'
       AND target.delivery_status IN ('pending', 'processing', 'sent') THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'sent', updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.delivered'
          AND target.delivery_status NOT IN ('bounced', 'complained', 'suppressed') THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'delivered', delivered_at = received_at, updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.bounced' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'bounced', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status NOT IN ('complained', 'suppressed');
        IF lower(COALESCE(payload #>> '{data,bounce,type}', '')) = 'permanent' THEN
            suppression_reason := 'hard_bounce';
        END IF;
    ELSIF provider_event_type = 'email.complained' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'complained', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status <> 'suppressed';
        suppression_reason := 'complaint';
    ELSIF provider_event_type = 'email.suppressed' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'suppressed', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id;
        suppression_reason := 'provider_suppression';
    END IF;

    IF suppression_reason IS NOT NULL THEN
        INSERT INTO notification.email_suppressions (
            id, merchant_account_id, store_id, recipient_email, suppression_reason,
            source_delivery_id, created_at, updated_at
        ) VALUES (
            uuidv7(), target.merchant_account_id, target.store_id, target.recipient_email,
            suppression_reason, target.id, received_at, received_at
        ) ON CONFLICT (merchant_account_id, store_id, recipient_email) DO UPDATE
            SET suppression_reason = CASE
                    WHEN notification.email_suppressions.suppression_reason = 'manual'
                        THEN notification.email_suppressions.suppression_reason
                    WHEN notification.email_suppressions.suppression_reason = 'complaint'
                        THEN notification.email_suppressions.suppression_reason
                    WHEN EXCLUDED.suppression_reason = 'complaint'
                        THEN EXCLUDED.suppression_reason
                    WHEN notification.email_suppressions.suppression_reason = 'hard_bounce'
                        THEN notification.email_suppressions.suppression_reason
                    ELSE EXCLUDED.suppression_reason
                END,
                source_delivery_id = CASE
                    WHEN notification.email_suppressions.suppression_reason IN ('manual', 'complaint')
                        THEN notification.email_suppressions.source_delivery_id
                    ELSE EXCLUDED.source_delivery_id
                END,
                updated_at = EXCLUDED.updated_at;
    END IF;
    RETURN true;
END;
$$;

CREATE FUNCTION fulfillment.claim_shipping_tracking(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    merchant_account_id UUID,
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
        FROM fulfillment.shipping_labels AS label
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
    UPDATE fulfillment.shipping_labels AS label
       SET tracking_locked_by = worker_id,
           tracking_locked_at = claimed_at,
           tracking_attempts = least(label.tracking_attempts, 30) + 1
      FROM claimable,
           fulfillment.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.merchant_account_id = label.merchant_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.merchant_account_id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_tracker_reference,
              account.credential_secret_reference, label.tracking_attempts;
$$;

CREATE FUNCTION fulfillment.claim_shipping_cancellations(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    merchant_account_id UUID,
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
        FROM fulfillment.shipping_labels AS label
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
    UPDATE fulfillment.shipping_labels AS label
       SET cancellation_locked_by = worker_id,
           cancellation_locked_at = claimed_at,
           cancellation_attempts = least(label.cancellation_attempts, 30) + 1
      FROM claimable,
           fulfillment.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.merchant_account_id = label.merchant_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.merchant_account_id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_shipment_reference,
              account.credential_secret_reference, label.cancellation_attempts;
$$;

CREATE FUNCTION analytics.claim_sessionization_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    behavior_event_id UUID,
    merchant_account_id UUID,
    store_id UUID,
    sales_channel_id UUID,
    event_name TEXT,
    anonymous_id UUID,
    client_session_id UUID,
    occurred_at TIMESTAMPTZ,
    retention_expires_at TIMESTAMPTZ,
    active_engagement_milliseconds INTEGER,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT job.id
          FROM analytics.behavior_event_processing AS job
         WHERE (
                   job.processing_status = 'pending'
                   AND job.available_at <= claimed_at
               )
            OR (
                   job.processing_status = 'processing'
                   AND job.locked_at <= stale_before
               )
         ORDER BY job.available_at, job.created_at, job.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    ), claimed AS (
        UPDATE analytics.behavior_event_processing AS job
           SET processing_status = 'processing',
               locked_by = worker_id,
               locked_at = claimed_at,
               attempts = least(job.attempts, 30) + 1,
               updated_at = claimed_at
          FROM claimable
         WHERE job.id = claimable.id
        RETURNING job.id, job.merchant_account_id, job.store_id, job.attempts
    )
    SELECT event.id,
           claimed.merchant_account_id,
           claimed.store_id,
           event.sales_channel_id,
           event.event_name::TEXT,
           event.anonymous_id,
           event.session_id,
           event.occurred_at,
           event.retention_expires_at,
           CASE WHEN event.event_name = 'engagement_heartbeat'
                THEN (event.properties ->> 'active_milliseconds')::INTEGER
                ELSE NULL
           END,
           claimed.attempts
      FROM claimed
      INNER JOIN analytics.behavior_events AS event
        ON event.merchant_account_id = claimed.merchant_account_id
       AND event.store_id = claimed.store_id
       AND event.id = claimed.id
     ORDER BY event.received_at, event.id;
$$;

CREATE FUNCTION merchant.authenticate_api_key(
    presented_key_identifier  TEXT,
    presented_secret_digest  BYTEA
)
RETURNS TABLE (
    api_key_id           UUID,
    merchant_account_id  UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    class                TEXT,
    mode                 TEXT,
    scopes               TEXT[]
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT api_key.id,
           api_key.merchant_account_id,
           api_key.store_id,
           sales_channel.id,
           api_key.class::TEXT,
           api_key.mode::TEXT,
           ARRAY(
               SELECT api_key_scope.scope::TEXT
               FROM merchant.api_key_scopes AS api_key_scope
               WHERE api_key_scope.merchant_account_id = api_key.merchant_account_id
                 AND api_key_scope.api_key_id = api_key.id
               ORDER BY api_key_scope.scope::TEXT
           )
    FROM merchant.api_keys AS api_key
    INNER JOIN merchant.merchant_accounts AS merchant_account
        ON merchant_account.id = api_key.merchant_account_id
    INNER JOIN merchant.stores AS store
        ON store.merchant_account_id = api_key.merchant_account_id
       AND store.id = api_key.store_id
    LEFT JOIN merchant.sales_channels AS sales_channel
        ON sales_channel.merchant_account_id = api_key.merchant_account_id
       AND sales_channel.store_id = api_key.store_id
       AND sales_channel.id = COALESCE(
           api_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM merchant.sales_channels AS default_channel
               WHERE default_channel.merchant_account_id = api_key.merchant_account_id
                 AND default_channel.store_id = api_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE api_key.key_identifier = presented_key_identifier
      AND api_key.secret_digest = presented_secret_digest
      AND api_key.revoked_at IS NULL
      AND (api_key.expires_at IS NULL OR api_key.expires_at > CURRENT_TIMESTAMP)
      AND merchant_account.status = 'active'
      AND (api_key.mode = 'test' OR store.status = 'active');
$$;

REVOKE ALL ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) FROM PUBLIC;
REVOKE ALL ON FUNCTION payments.resolve_provider_account(TEXT, TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION payments.resolve_provider_webhook_secret_references(TEXT, TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION payments.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION payments.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_fulfillment_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.claim_commerce_fact_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.claim_attribution_jobs(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.rebuild_store_attribution(
    UUID, UUID, SMALLINT, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION sales.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_webhook_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_outbox_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_webhook_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION notification.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION notification.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION notification.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION fulfillment.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION fulfillment.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.claim_sessionization_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.event_consumer_backlog() FROM PUBLIC;
REVOKE ALL ON FUNCTION payments.provider_readiness_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION notification.email_delivery_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION fulfillment.shipping_tracking_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION fulfillment.shipping_cancellation_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.sessionization_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.attribution_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.retention_metrics() FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.apply_store_retention_policy(UUID, UUID, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.purge_expired_data(INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.process_erasure_requests(INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION analytics.erasure_metrics() FROM PUBLIC;

COMMENT ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) IS
    'Authenticates a machine credential without exposing stored secret digests';

DO $$
BEGIN
    CREATE ROLE chaos_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    CREATE ROLE chaos_control_plane NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    EXECUTE format('GRANT chaos_runtime TO %I', current_user);
    EXECUTE format('GRANT chaos_control_plane TO %I', current_user);
END
$$;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

GRANT USAGE ON SCHEMA extensions, integration, merchant, catalog, pricing, inventory, sales,
    payments, fulfillment, notification, analytics, search
    TO chaos_runtime;
GRANT EXECUTE
    ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION payments.resolve_provider_account(TEXT, TEXT) TO chaos_runtime;
GRANT EXECUTE
    ON FUNCTION payments.resolve_provider_webhook_secret_references(TEXT, TEXT) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION payments.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION payments.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_fulfillment_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.claim_commerce_fact_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.claim_attribution_jobs(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.rebuild_store_attribution(
    UUID, UUID, SMALLINT, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION sales.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_webhook_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_outbox_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_webhook_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION notification.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION notification.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION notification.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION fulfillment.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION fulfillment.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.claim_sessionization_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.queue_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.event_consumer_backlog() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION payments.provider_readiness_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION notification.email_delivery_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION fulfillment.shipping_tracking_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION fulfillment.shipping_cancellation_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.sessionization_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.attribution_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.retention_metrics() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.apply_store_retention_policy(UUID, UUID, INTEGER)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.purge_expired_data(INTEGER, TIMESTAMPTZ)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.process_erasure_requests(INTEGER, TIMESTAMPTZ)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION analytics.erasure_metrics() TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON integration.event_consumer_registry FROM chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA merchant TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA catalog TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA pricing TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA inventory TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA sales TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA payments TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA fulfillment TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA notification TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA analytics TO chaos_runtime;
REVOKE UPDATE, DELETE ON analytics.store_policy_versions FROM chaos_runtime;
REVOKE UPDATE, DELETE ON analytics.identity_links FROM chaos_runtime;
REVOKE UPDATE, DELETE ON analytics.behavior_events FROM chaos_runtime;
REVOKE UPDATE, DELETE ON analytics.erasure_requests FROM chaos_runtime;
REVOKE UPDATE, DELETE ON analytics.commerce_facts FROM chaos_runtime;
GRANT SELECT ON ALL TABLES IN SCHEMA search TO chaos_runtime;
GRANT EXECUTE ON FUNCTION search.rebuild_store_products(UUID, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION search.process_events(UUID, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
REVOKE UPDATE, DELETE
    ON inventory.stock_ledger_entries FROM chaos_runtime;
REVOKE UPDATE, DELETE
    ON sales.customer_shopper_links FROM chaos_runtime;
REVOKE UPDATE, DELETE
    ON sales.checkout_contacts, sales.checkout_addresses, sales.checkout_lines,
       sales.checkout_shipping_selections, sales.checkout_tax_calculations,
       sales.checkout_promotion_calculations FROM chaos_runtime;
REVOKE UPDATE, DELETE
    ON sales.order_contacts, sales.order_addresses, sales.order_lines,
       sales.order_shipping_selections, sales.order_tax_calculations,
       sales.order_promotion_calculations, sales.order_transitions,
       sales.order_fulfillment_transitions
    FROM chaos_runtime;
REVOKE DELETE ON sales.checkouts, sales.orders FROM chaos_runtime;
REVOKE UPDATE, DELETE
    ON integration.webhook_inbox, integration.outbox_events FROM chaos_runtime;
REVOKE UPDATE, DELETE
    ON notification.email_deliveries, notification.email_suppressions,
       notification.webhook_events FROM chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA merchant TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA catalog TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA pricing TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA inventory TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA sales TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA payments TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA fulfillment TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA notification TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA catalog
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA catalog
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA sales
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA sales
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA payments
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA payments
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA fulfillment
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA fulfillment
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA analytics
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA search
    GRANT SELECT ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA extensions, identity TO chaos_control_plane;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA identity TO chaos_control_plane;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA identity TO chaos_control_plane;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_control_plane;
ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_control_plane;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';
COMMENT ON ROLE chaos_control_plane IS
    'Non-owner identity role. It cannot access merchant-owned tables.';
