CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');
CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_payment_status AS ENUM ('pending', 'paid', 'failed', 'partially_refunded', 'refunded');
CREATE TYPE commerce.order_shipping_status AS ENUM ('pending', 'shipped', 'delivered', 'cancelled');

-- TODO: remove first_seen_at
CREATE TABLE commerce.shoppers (
    id               UUID                  NOT NULL PRIMARY KEY,
    store_id         UUID                  NOT NULL,
    first_seen_at    TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at     TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at       TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT shoppers_store_id_id_key    UNIQUE (store_id, id),
    CONSTRAINT shoppers_store_id_fkey      FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE
);

CREATE TABLE commerce.carts (
    id                   UUID                    NOT NULL PRIMARY KEY,
    store_id             UUID                    NOT NULL,
    sales_channel_id     UUID                    NOT NULL,
    shopper_id           UUID                    NOT NULL,
    price_list_id        UUID                    NOT NULL,
    currency             CHAR(3)                 NOT NULL,
    status               commerce.cart_status    NOT NULL DEFAULT 'active',
    version              BIGINT                  NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT carts_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT carts_store_id_id_shopper_id_key        UNIQUE (store_id, id, shopper_id),
    CONSTRAINT carts_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT carts_sales_channel_fkey                FOREIGN KEY (sales_channel_id) REFERENCES commerce.store_sales_channels(id),
    CONSTRAINT carts_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers(store_id, id),
    CONSTRAINT carts_store_id_price_list_currency_fkey FOREIGN KEY (store_id, price_list_id, currency) REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT carts_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT carts_version_nonnegative_check         CHECK (version >= 0)
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

    CONSTRAINT cart_lines_pkey                          PRIMARY KEY (store_id, cart_id, product_variant_id),
    CONSTRAINT cart_lines_store_id_cart_fkey            FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts(store_id, id) ON DELETE CASCADE,
    CONSTRAINT cart_lines_store_id_product_variant_fkey FOREIGN KEY (store_id, product_id, product_variant_id) REFERENCES commerce.product_variants(store_id, product_id, id),
    CONSTRAINT cart_lines_product_title_length_check    CHECK (length(trim(product_title)) BETWEEN 1 AND 255),
    CONSTRAINT cart_lines_variant_title_length_check    CHECK (length(trim(variant_title)) BETWEEN 1 AND 255),
    CONSTRAINT cart_lines_sku_length_check              CHECK (sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64),
    CONSTRAINT cart_lines_quantity_range_check          CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT cart_lines_unit_price_nonnegative_check  CHECK (unit_price_amount_minor >= 0)
);

CREATE TABLE commerce.orders (
    id                           UUID                               NOT NULL PRIMARY KEY,
    store_id                     UUID                               NOT NULL,
    order_number                 TEXT                               NOT NULL,
    sales_channel_id             UUID                               NOT NULL,
    cart_id                      UUID                               NOT NULL,
    shopper_id                   UUID                               NOT NULL,
    price_list_id                UUID                               NOT NULL,
    currency                     CHAR(3)                            NOT NULL,
    status                       commerce.order_status              NOT NULL DEFAULT 'pending',
    payment_status               commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    shipping_status              commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    stripe_checkout_session_id   TEXT,
    stripe_payment_intent_id     TEXT,
    stripe_charge_id             TEXT,
    stripe_refund_id             TEXT,
    payment_failure_code         TEXT,
    refunded_amount_minor        BIGINT                             NOT NULL DEFAULT 0,
    shipping_provider            TEXT,
    shipping_provider_reference  TEXT,
    shipping_tracking_number     TEXT,
    shipping_tracking_url        TEXT,
    subtotal_amount_minor        BIGINT                             NOT NULL,
    discount_amount_minor        BIGINT                             NOT NULL,
    tax_amount_minor             BIGINT                             NOT NULL,
    shipping_amount_minor        BIGINT                             NOT NULL,
    total_amount_minor           BIGINT                             NOT NULL,
    contact_email                extensions.citext,
    contact_phone                TEXT,
    billing_full_name            TEXT,
    billing_company              TEXT,
    billing_address_line1        TEXT,
    billing_address_line2        TEXT,
    billing_locality             TEXT,
    billing_administrative_area  TEXT,
    billing_postal_code          TEXT,
    billing_country_code         CHAR(2),
    shipping_full_name           TEXT,
    shipping_company             TEXT,
    shipping_address_line1       TEXT,
    shipping_address_line2       TEXT,
    shipping_locality            TEXT,
    shipping_administrative_area TEXT,
    shipping_postal_code         TEXT,
    shipping_country_code        CHAR(2),
    created_at                   TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT orders_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT orders_store_id_order_number_key         UNIQUE (store_id, order_number),
    CONSTRAINT orders_store_id_id_shopper_id_key        UNIQUE (store_id, id, shopper_id),
    CONSTRAINT orders_store_id_cart_fkey                FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts(store_id, id),
    CONSTRAINT orders_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers(store_id, id),
    CONSTRAINT orders_sales_channel_fkey                FOREIGN KEY (sales_channel_id) REFERENCES commerce.store_sales_channels(id),
    CONSTRAINT orders_store_id_price_list_currency_fkey FOREIGN KEY (store_id, price_list_id, currency) REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT orders_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_order_number_check                CHECK (order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'),
    CONSTRAINT orders_amounts_check                     CHECK (subtotal_amount_minor >= 0 AND discount_amount_minor >= 0 AND tax_amount_minor >= 0 AND shipping_amount_minor >= 0 AND total_amount_minor >= 0 AND refunded_amount_minor >= 0 AND refunded_amount_minor <= total_amount_minor),
    CONSTRAINT orders_contact_email_length_check        CHECK (contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320),
    CONSTRAINT orders_contact_phone_format_check        CHECK (contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'),
    CONSTRAINT orders_billing_country_code_check        CHECK (billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_shipping_country_code_check       CHECK (shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_stripe_checkout_session_check     CHECK (stripe_checkout_session_id IS NULL OR length(trim(stripe_checkout_session_id)) BETWEEN 1 AND 255),
    CONSTRAINT orders_stripe_payment_intent_check       CHECK (stripe_payment_intent_id IS NULL OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'),
    CONSTRAINT orders_stripe_charge_check               CHECK (stripe_charge_id IS NULL OR stripe_charge_id ~ '^ch_[A-Za-z0-9]+$'),
    CONSTRAINT orders_stripe_refund_check               CHECK (stripe_refund_id IS NULL OR stripe_refund_id ~ '^re_[A-Za-z0-9]+$'),
    CONSTRAINT orders_payment_failure_code_check        CHECK (payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT orders_shipping_provider_check           CHECK (shipping_provider IS NULL OR length(trim(shipping_provider)) BETWEEN 1 AND 64),
    CONSTRAINT orders_shipping_reference_check          CHECK (shipping_provider_reference IS NULL OR length(trim(shipping_provider_reference)) BETWEEN 1 AND 255),
    CONSTRAINT orders_shipping_tracking_number_check    CHECK (shipping_tracking_number IS NULL OR length(trim(shipping_tracking_number)) BETWEEN 1 AND 255),
    CONSTRAINT orders_shipping_tracking_url_check       CHECK (shipping_tracking_url IS NULL OR (length(shipping_tracking_url) BETWEEN 9 AND 2048 AND shipping_tracking_url ~ '^https://'))
);

CREATE TABLE commerce.order_tracking_tokens (
    store_id       UUID        NOT NULL,
    order_id       UUID        NOT NULL,
    token_digest   BYTEA       NOT NULL,
    expires_at     TIMESTAMPTZ NOT NULL,
    last_used_at   TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT order_tracking_tokens_pkey                PRIMARY KEY (store_id, order_id),
    CONSTRAINT order_tracking_tokens_store_id_token_key  UNIQUE (store_id, token_digest),
    CONSTRAINT order_tracking_tokens_store_id_order_fkey FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_tokens_digest_check        CHECK (octet_length(token_digest) = 32),
    CONSTRAINT order_tracking_tokens_expiry_check        CHECK (expires_at > created_at)
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

    CONSTRAINT order_lines_pkey                          PRIMARY KEY (store_id, order_id, position),
    CONSTRAINT order_lines_store_id_order_id_variant_key UNIQUE (store_id, order_id, product_variant_id),
    CONSTRAINT order_lines_store_id_order_fkey           FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    CONSTRAINT order_lines_position_check                CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT order_lines_product_title_length_check    CHECK (length(trim(product_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_variant_title_length_check    CHECK (length(trim(variant_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_sku_length_check              CHECK (sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64),
    CONSTRAINT order_lines_quantity_range_check          CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT order_lines_amounts_check                 CHECK (unit_price_amount_minor >= 0 AND subtotal_amount_minor = unit_price_amount_minor * quantity AND subtotal_amount_minor >= 0)
);

CREATE TABLE commerce.order_transitions (
    id                   UUID                             NOT NULL PRIMARY KEY,
    store_id             UUID                             NOT NULL,
    order_id             UUID                             NOT NULL,
    from_status          commerce.order_status,
    to_status            commerce.order_status            NOT NULL,
    kind                 commerce.order_transition_kind   NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                      NOT NULL,

    CONSTRAINT order_transitions_store_id_order_id_id_key UNIQUE (store_id, order_id, id),
    CONSTRAINT order_transitions_store_id_order_fkey      FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    CONSTRAINT order_transitions_actor_user_fkey          FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT order_transitions_shape_check              CHECK ((kind = 'created' AND from_status IS NULL AND to_status = 'pending') OR (kind = 'confirmed' AND from_status = 'pending' AND to_status = 'confirmed') OR (kind = 'cancelled' AND from_status = 'pending' AND to_status = 'cancelled'))
);

ALTER TABLE commerce.orders ADD UNIQUE (store_id, id, currency);

CREATE INDEX shoppers_store_seen_idx ON commerce.shoppers (store_id, last_seen_at DESC, id DESC);
CREATE INDEX carts_channel_updated_idx ON commerce.carts (store_id, sales_channel_id, status, updated_at DESC, id DESC);
CREATE INDEX cart_lines_variant_lookup_idx ON commerce.cart_lines (store_id, product_variant_id, cart_id);
CREATE INDEX orders_channel_created_idx ON commerce.orders (store_id, sales_channel_id, created_at DESC, id DESC);
CREATE INDEX order_tracking_tokens_expiry_idx ON commerce.order_tracking_tokens (expires_at, store_id, order_id);
CREATE INDEX order_transitions_order_time_idx ON commerce.order_transitions (store_id, order_id, occurred_at, id);
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
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.cart_lines
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.orders
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_tracking_tokens
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_lines
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_transitions
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.shoppers,
       commerce.carts,
       commerce.cart_lines,
       commerce.orders,
       commerce.order_tracking_tokens,
       commerce.order_lines,
       commerce.order_transitions
    TO chaos_runtime;

REVOKE DELETE ON commerce.orders FROM chaos_runtime;
REVOKE UPDATE, DELETE ON commerce.order_lines, commerce.order_transitions FROM chaos_runtime;
