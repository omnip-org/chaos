CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');
CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_payment_status AS ENUM ('pending', 'paid', 'failed', 'partially_refunded', 'refunded');
CREATE TYPE commerce.order_shipping_status AS ENUM ('pending', 'awaiting_pickup', 'shipped', 'delivered', 'cancelled');

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

-- A Cart has no currency of its own: a Store trades in exactly one currency
-- (`stores.currency`), and every Price List row a Cart can bind to already
-- carries that same currency. Readers resolve display currency through
-- `price_list_id`, not through a redundant column here.
CREATE TABLE commerce.carts (
    id                   UUID                    NOT NULL PRIMARY KEY,
    store_id             UUID                    NOT NULL,
    sales_channel_id     UUID                    NOT NULL,
    shopper_id           UUID                    NOT NULL,
    price_list_id        UUID                    NOT NULL,
    status               commerce.cart_status    NOT NULL DEFAULT 'active',
    version              BIGINT                  NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT carts_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT carts_store_id_id_shopper_id_key        UNIQUE (store_id, id, shopper_id),
    CONSTRAINT carts_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT carts_sales_channel_fkey                FOREIGN KEY (sales_channel_id) REFERENCES commerce.store_sales_channels(id),
    CONSTRAINT carts_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers(store_id, id),
    CONSTRAINT carts_store_id_price_list_fkey          FOREIGN KEY (store_id, price_list_id) REFERENCES commerce.price_lists(store_id, id),
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
    request_id                   UUID                               NOT NULL,
    price_list_id                UUID                               NOT NULL,
    currency                     CHAR(3)                            NOT NULL,
    status                       commerce.order_status              NOT NULL DEFAULT 'pending',
    -- payment_status and refunded_amount_minor are projections recomputed
    -- from commerce.refunds on every refund event; stripe_payment_intent_id
    -- and payment_failure_code are the Order's own payment reference — a
    -- failed or expired Stripe Checkout Session always cancels the Order
    -- rather than allowing a retry, so there is only ever one attempt to
    -- track per Order and it lives directly on this row. Partial refunds
    -- still get their own identity — see commerce.refunds.
    payment_status               commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    stripe_payment_intent_id     TEXT,
    payment_failure_code         TEXT,
    -- shipping_status is likewise a projection derived from
    -- commerce.fulfillments; the shipment's own provider, tracking number,
    -- and tracking URL live on that table, one row per shipment.
    shipping_status              commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    refunded_amount_minor        BIGINT                             NOT NULL DEFAULT 0,
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
    CONSTRAINT orders_store_id_sales_channel_id_shopper_id_request_id_key UNIQUE (store_id, sales_channel_id, shopper_id, request_id),
    CONSTRAINT orders_store_id_cart_fkey                FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts(store_id, id),
    CONSTRAINT orders_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers(store_id, id),
    CONSTRAINT orders_sales_channel_fkey                FOREIGN KEY (sales_channel_id) REFERENCES commerce.store_sales_channels(id),
    CONSTRAINT orders_store_id_price_list_currency_fkey FOREIGN KEY (store_id, price_list_id, currency) REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT orders_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_request_id_not_nil_check           CHECK (request_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT orders_order_number_check                CHECK (order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'),
    CONSTRAINT orders_amounts_check                     CHECK (subtotal_amount_minor >= 0 AND discount_amount_minor >= 0 AND tax_amount_minor >= 0 AND shipping_amount_minor >= 0 AND total_amount_minor >= 0 AND refunded_amount_minor >= 0 AND refunded_amount_minor <= total_amount_minor),
    CONSTRAINT orders_contact_email_length_check        CHECK (contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320),
    CONSTRAINT orders_contact_phone_format_check        CHECK (contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'),
    CONSTRAINT orders_billing_country_code_check        CHECK (billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_shipping_country_code_check       CHECK (shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_store_id_payment_intent_key       UNIQUE (store_id, stripe_payment_intent_id),
    CONSTRAINT orders_payment_intent_check              CHECK (stripe_payment_intent_id IS NULL OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'),
    CONSTRAINT orders_payment_failure_code_check        CHECK (payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000)
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
    occurred_at          TIMESTAMPTZ                      NOT NULL,

    CONSTRAINT order_transitions_store_id_order_id_id_key UNIQUE (store_id, order_id, id),
    CONSTRAINT order_transitions_store_id_order_fkey      FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
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
