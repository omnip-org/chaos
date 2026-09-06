CREATE TYPE commerce.cart_status AS ENUM ('active', 'locked', 'completed', 'abandoned');
CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_payment_status AS ENUM ('pending', 'paid', 'failed', 'expired', 'partially_refunded', 'refunded');
CREATE TYPE commerce.order_refund_status AS ENUM ('pending', 'succeeded', 'failed');
CREATE TYPE commerce.order_fulfillment_status AS ENUM ('pending', 'shipped', 'delivered', 'cancelled');

CREATE TABLE commerce.carts (
    id                    UUID                    NOT NULL PRIMARY KEY,
    store_id              UUID                    NOT NULL,
    channel_id            UUID                    NOT NULL,
    shopper_id            UUID                    NOT NULL,
    price_list_id         UUID                    NOT NULL,
    status                commerce.cart_status    NOT NULL DEFAULT 'active',
    payment_client_action JSONB,
    attribution           JSONB,
    checkout_idempotency_key     UUID,
    checkout_request_fingerprint BYTEA,
    created_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT carts_store_id_id_key              UNIQUE (store_id, id),
    CONSTRAINT carts_store_id_id_channel_id_shopper_id_price_list_id_key UNIQUE (store_id, id, channel_id, shopper_id, price_list_id),
    CONSTRAINT carts_store_id_fkey                FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT carts_store_id_channel_fkey        FOREIGN KEY (store_id, channel_id) REFERENCES commerce.channels (store_id, id),
    CONSTRAINT carts_store_id_shopper_fkey        FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers (store_id, id),
    CONSTRAINT carts_store_id_price_list_fkey     FOREIGN KEY (store_id, price_list_id) REFERENCES commerce.price_lists (store_id, id),
    CONSTRAINT carts_payment_client_action_check  CHECK (
        payment_client_action IS NULL
        OR (
            status = 'locked'
            AND jsonb_typeof(payment_client_action) = 'object'
            AND payment_client_action ? 'type'
            AND jsonb_typeof(payment_client_action->'type') = 'string'
            AND pg_column_size(payment_client_action) <= 8192
        )
    ),
    CONSTRAINT carts_attribution_check            CHECK (
        attribution IS NULL
        OR (jsonb_typeof(attribution) = 'object' AND pg_column_size(attribution) <= 4096)
    ),
    -- Checkout-request idempotency lives on the Cart, not the Order: it is
    -- keyed by the checkout attempt (which targets a Cart) and there is
    -- exactly one Order per Cart. Stamped when the Cart leaves 'active' and
    -- never cleared or re-activated, so it is set iff a checkout has started.
    CONSTRAINT carts_checkout_idempotency_key_check     CHECK (
        checkout_idempotency_key IS NULL
        OR (status <> 'active'
            AND checkout_idempotency_key <> '00000000-0000-0000-0000-000000000000'::uuid)
    ),
    CONSTRAINT carts_checkout_request_fingerprint_check CHECK (
        checkout_request_fingerprint IS NULL
        OR octet_length(checkout_request_fingerprint) = 32
    )
);

CREATE TABLE commerce.cart_lines (
    store_id           UUID        NOT NULL,
    cart_id            UUID        NOT NULL,
    product_variant_id UUID        NOT NULL,
    quantity           INTEGER     NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT cart_lines_pkey                  PRIMARY KEY (store_id, cart_id, product_variant_id),
    CONSTRAINT cart_lines_store_id_cart_fkey    FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts (store_id, id) ON DELETE CASCADE,
    CONSTRAINT cart_lines_store_id_variant_fkey FOREIGN KEY (store_id, product_variant_id) REFERENCES commerce.product_variants (store_id, id),
    CONSTRAINT cart_lines_quantity_range_check  CHECK (quantity BETWEEN 1 AND 999)
);

CREATE TABLE commerce.orders (
    id                              UUID                            NOT NULL PRIMARY KEY,
    order_number                    TEXT                            NOT NULL,
    store_id                        UUID                            NOT NULL,
    channel_id                      UUID                            NOT NULL,
    shopper_id                      UUID                            NOT NULL,
    cart_id                         UUID                            NOT NULL,
    price_list_id                   UUID                            NOT NULL,
    currency                        CHAR(3)                         NOT NULL,
    status                          commerce.order_status           NOT NULL DEFAULT 'pending',
    payment_status                  commerce.order_payment_status   NOT NULL DEFAULT 'pending',
    payment_provider_account_id     UUID                            NOT NULL,
    payment_provider_reference_id   TEXT,
    payment_failure_code            TEXT,
    fulfillment_status              commerce.order_fulfillment_status  NOT NULL DEFAULT 'pending',
    refunded_amount_minor           BIGINT                          NOT NULL DEFAULT 0,
    subtotal_amount_minor           BIGINT                          NOT NULL,
    discount_amount_minor           BIGINT                          NOT NULL,
    tax_amount_minor                BIGINT                          NOT NULL,
    shipping_amount_minor           BIGINT                          NOT NULL,
    total_amount_minor              BIGINT                          NOT NULL,
    amounts_finalized_at            TIMESTAMPTZ,
    contact_email                   extensions.citext,
    contact_phone                   TEXT,
    billing_full_name               TEXT,
    billing_address_line1           TEXT,
    billing_address_line2           TEXT,
    billing_locality                TEXT,
    billing_administrative_area     TEXT,
    billing_postal_code             TEXT,
    billing_country_code            CHAR(2),
    shipping_full_name              TEXT,
    shipping_address_line1          TEXT,
    shipping_address_line2          TEXT,
    shipping_locality               TEXT,
    shipping_administrative_area    TEXT,
    shipping_postal_code            TEXT,
    shipping_country_code           CHAR(2),
    created_at                      TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT orders_store_id_id_key                         UNIQUE (store_id, id),
    CONSTRAINT orders_store_id_order_number_key               UNIQUE (store_id, order_number),
    CONSTRAINT orders_store_id_id_currency_key                UNIQUE (store_id, id, currency),
    CONSTRAINT orders_store_cart_context_fkey                 FOREIGN KEY (store_id, cart_id, channel_id, shopper_id, price_list_id) REFERENCES commerce.carts (store_id, id, channel_id, shopper_id, price_list_id),
    CONSTRAINT orders_store_id_price_list_currency_fkey       FOREIGN KEY (store_id, price_list_id, currency) REFERENCES commerce.price_lists (store_id, id, currency),
    CONSTRAINT orders_store_id_payment_provider_account_fkey  FOREIGN KEY (store_id, payment_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT orders_currency_format_check                   CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_order_number_check                      CHECK (order_number ~ '^W-[0-9A-HJKMNP-TV-Z]{8}$'),
    CONSTRAINT orders_amounts_check                           CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor >= 0
        AND refunded_amount_minor >= 0
        AND refunded_amount_minor <= total_amount_minor
        AND (
            (
                amounts_finalized_at IS NULL
                AND discount_amount_minor = 0
                AND tax_amount_minor = 0
                AND shipping_amount_minor = 0
                AND total_amount_minor = 0
            )
            OR (
                amounts_finalized_at IS NOT NULL
                AND total_amount_minor::numeric = subtotal_amount_minor::numeric
                    - discount_amount_minor::numeric
                    + tax_amount_minor::numeric
                    + shipping_amount_minor::numeric
            )
        )
    ),
    CONSTRAINT orders_contact_email_length_check              CHECK (contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320),
    CONSTRAINT orders_contact_phone_format_check              CHECK (contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'),
    CONSTRAINT orders_billing_country_code_check              CHECK (billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_shipping_country_code_check             CHECK (shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_payment_provider_reference_check        CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT orders_payment_failure_code_check              CHECK (payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT orders_payment_failure_code_shape_check        CHECK (payment_failure_code IS NULL OR payment_status IN ('failed', 'expired')),
    CONSTRAINT orders_refund_state_check                      CHECK (
        (refunded_amount_minor = 0 AND payment_status NOT IN ('partially_refunded', 'refunded'))
        OR (refunded_amount_minor > 0 AND payment_status IN ('partially_refunded', 'refunded'))
    ),
    CONSTRAINT orders_billing_address_shape_check             CHECK (
        (
            billing_full_name IS NULL
            AND billing_address_line1 IS NULL
            AND billing_address_line2 IS NULL
            AND billing_locality IS NULL
            AND billing_administrative_area IS NULL
            AND billing_postal_code IS NULL
            AND billing_country_code IS NULL
        )
        OR (
            billing_full_name IS NOT NULL
            AND length(trim(billing_full_name)) BETWEEN 1 AND 200
            AND billing_full_name !~ '[[:cntrl:]]'
            AND billing_address_line1 IS NOT NULL
            AND length(trim(billing_address_line1)) BETWEEN 1 AND 255
            AND billing_address_line1 !~ '[[:cntrl:]]'
            AND billing_locality IS NOT NULL
            AND length(trim(billing_locality)) BETWEEN 1 AND 100
            AND billing_locality !~ '[[:cntrl:]]'
            AND billing_country_code IS NOT NULL
            AND (
                billing_address_line2 IS NULL
                OR (
                    length(trim(billing_address_line2)) BETWEEN 1 AND 255
                    AND billing_address_line2 !~ '[[:cntrl:]]'
                )
            )
            AND (
                billing_administrative_area IS NULL
                OR (
                    length(trim(billing_administrative_area)) BETWEEN 1 AND 100
                    AND billing_administrative_area !~ '[[:cntrl:]]'
                )
            )
            AND (
                billing_postal_code IS NULL
                OR (
                    length(trim(billing_postal_code)) BETWEEN 1 AND 32
                    AND billing_postal_code !~ '[[:cntrl:]]'
                )
            )
        )
    ),
    CONSTRAINT orders_shipping_address_shape_check            CHECK (
        (
            shipping_full_name IS NULL
            AND shipping_address_line1 IS NULL
            AND shipping_address_line2 IS NULL
            AND shipping_locality IS NULL
            AND shipping_administrative_area IS NULL
            AND shipping_postal_code IS NULL
            AND shipping_country_code IS NULL
        )
        OR (
            shipping_full_name IS NOT NULL
            AND length(trim(shipping_full_name)) BETWEEN 1 AND 200
            AND shipping_full_name !~ '[[:cntrl:]]'
            AND shipping_address_line1 IS NOT NULL
            AND length(trim(shipping_address_line1)) BETWEEN 1 AND 255
            AND shipping_address_line1 !~ '[[:cntrl:]]'
            AND shipping_locality IS NOT NULL
            AND length(trim(shipping_locality)) BETWEEN 1 AND 100
            AND shipping_locality !~ '[[:cntrl:]]'
            AND shipping_country_code IS NOT NULL
            AND (
                shipping_address_line2 IS NULL
                OR (
                    length(trim(shipping_address_line2)) BETWEEN 1 AND 255
                    AND shipping_address_line2 !~ '[[:cntrl:]]'
                )
            )
            AND (
                shipping_administrative_area IS NULL
                OR (
                    length(trim(shipping_administrative_area)) BETWEEN 1 AND 100
                    AND shipping_administrative_area !~ '[[:cntrl:]]'
                )
            )
            AND (
                shipping_postal_code IS NULL
                OR (
                    length(trim(shipping_postal_code)) BETWEEN 1 AND 32
                    AND shipping_postal_code !~ '[[:cntrl:]]'
                )
            )
        )
    )
);

CREATE TABLE commerce.order_lines (
    store_id                UUID        NOT NULL,
    order_id                UUID        NOT NULL,
    position                SMALLINT    NOT NULL,
    product_id              UUID        NOT NULL,
    product_variant_id      UUID        NOT NULL,
    product_title           TEXT        NOT NULL,
    variant_title           TEXT        NOT NULL,
    sku                     TEXT,
    track_inventory         BOOLEAN     NOT NULL,
    quantity                INTEGER     NOT NULL,
    unit_price_amount_minor BIGINT      NOT NULL,
    subtotal_amount_minor   BIGINT      NOT NULL,
    image_url               TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT order_lines_pkey                          PRIMARY KEY (store_id, order_id, position),
    CONSTRAINT order_lines_store_id_order_id_variant_key UNIQUE (store_id, order_id, product_variant_id),
    CONSTRAINT order_lines_store_id_order_fkey           FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id),
    CONSTRAINT order_lines_position_check                CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT order_lines_product_title_length_check    CHECK (length(trim(product_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_variant_title_length_check    CHECK (length(trim(variant_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_sku_length_check              CHECK (sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64),
    CONSTRAINT order_lines_quantity_range_check          CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT order_lines_amounts_check                 CHECK (unit_price_amount_minor >= 0 AND subtotal_amount_minor = unit_price_amount_minor * quantity AND subtotal_amount_minor >= 0),
    CONSTRAINT order_lines_image_url_check               CHECK (image_url IS NULL OR (length(image_url) BETWEEN 9 AND 2048 AND image_url ~ '^https://'))
);

CREATE TABLE commerce.order_refunds (
    id                            UUID                         NOT NULL PRIMARY KEY,
    store_id                      UUID                         NOT NULL,
    order_id                      UUID                         NOT NULL,
    currency                      CHAR(3)                      NOT NULL,
    status                        commerce.order_refund_status NOT NULL DEFAULT 'pending',
    amount_minor                  BIGINT                       NOT NULL,
    payment_provider_account_id   UUID                         NOT NULL,
    payment_provider_reference_id TEXT,
    failure_code                  TEXT,
    created_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT refunds_store_id_id_key                         UNIQUE (store_id, id),
    CONSTRAINT refunds_store_id_order_currency_fkey            FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders (store_id, id, currency),
    CONSTRAINT refunds_store_id_payment_provider_account_fkey  FOREIGN KEY (store_id, payment_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT refunds_amount_positive_check                   CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check                   CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_payment_provider_reference_check        CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT refunds_failure_code_check                      CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_failure_code_shape_check                CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE TABLE commerce.order_fulfillments (
    id                     UUID                               NOT NULL PRIMARY KEY,
    store_id               UUID                               NOT NULL,
    order_id               UUID                               NOT NULL,
    status                 commerce.order_fulfillment_status  NOT NULL DEFAULT 'pending',
    provider_account_id    UUID                               NOT NULL,
    provider_reference_id  TEXT,
    tracking_number        TEXT,
    tracking_url           TEXT,
    shipped_at             TIMESTAMPTZ,
    delivered_at           TIMESTAMPTZ,
    cancelled_at           TIMESTAMPTZ,
    created_at             TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT fulfillments_store_id_id_key                            UNIQUE (store_id, id),
    CONSTRAINT fulfillments_store_id_order_fkey                        FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id),
    CONSTRAINT fulfillments_store_id_provider_account_fkey             FOREIGN KEY (store_id, provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT fulfillments_provider_reference_check                   CHECK (provider_reference_id IS NULL OR length(trim(provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT fulfillments_tracking_number_check                      CHECK (tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255),
    CONSTRAINT fulfillments_tracking_url_check                         CHECK (tracking_url IS NULL OR (length(tracking_url) BETWEEN 9 AND 2048 AND tracking_url ~ '^https://')),
    CONSTRAINT fulfillments_shape_check                                CHECK (
        (status = 'pending' AND shipped_at IS NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'shipped' AND shipped_at IS NOT NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'delivered' AND shipped_at IS NOT NULL AND delivered_at IS NOT NULL AND cancelled_at IS NULL) OR
        (status = 'cancelled' AND cancelled_at IS NOT NULL)
    )
);

CREATE INDEX carts_channel_updated_idx ON commerce.carts (store_id, channel_id, status, updated_at DESC, id DESC);
CREATE UNIQUE INDEX carts_one_active_per_shopper_key ON commerce.carts (store_id, channel_id, shopper_id) WHERE status = 'active';
CREATE UNIQUE INDEX carts_checkout_idempotency_key_key ON commerce.carts (store_id, channel_id, shopper_id, checkout_idempotency_key) WHERE checkout_idempotency_key IS NOT NULL;
CREATE INDEX carts_store_shopper_idx ON commerce.carts (store_id, shopper_id, id);
CREATE INDEX carts_store_price_list_idx ON commerce.carts (store_id, price_list_id, id);
CREATE INDEX cart_lines_variant_lookup_idx ON commerce.cart_lines (store_id, product_variant_id, cart_id);
CREATE INDEX orders_channel_created_idx ON commerce.orders (store_id, channel_id, created_at DESC, id DESC);
CREATE INDEX orders_store_status_id_idx ON commerce.orders (store_id, status, id DESC);
CREATE INDEX orders_store_contact_email_id_idx ON commerce.orders (store_id, contact_email, id DESC) WHERE contact_email IS NOT NULL;
CREATE UNIQUE INDEX orders_one_order_per_cart_key ON commerce.orders (store_id, cart_id);
CREATE INDEX orders_store_shopper_idx ON commerce.orders (store_id, shopper_id);
CREATE INDEX orders_store_price_list_currency_idx ON commerce.orders (store_id, price_list_id, currency);
CREATE UNIQUE INDEX orders_payment_provider_reference_key ON commerce.orders (store_id, payment_provider_account_id, payment_provider_reference_id) WHERE payment_provider_reference_id IS NOT NULL;
CREATE UNIQUE INDEX fulfillments_provider_reference_key ON commerce.order_fulfillments (store_id, provider_account_id, provider_reference_id) WHERE provider_reference_id IS NOT NULL;
CREATE INDEX refunds_order_created_idx ON commerce.order_refunds (store_id, order_id, created_at DESC);
CREATE INDEX refunds_payment_provider_account_idx ON commerce.order_refunds (store_id, payment_provider_account_id, order_id);
CREATE UNIQUE INDEX refunds_payment_provider_reference_key ON commerce.order_refunds (store_id, payment_provider_account_id, payment_provider_reference_id) WHERE payment_provider_reference_id IS NOT NULL;
CREATE INDEX fulfillments_order_created_idx ON commerce.order_fulfillments (store_id, order_id, created_at DESC);
CREATE INDEX fulfillments_provider_account_idx ON commerce.order_fulfillments (store_id, provider_account_id, order_id);
CREATE INDEX orders_payment_provider_account_idx ON commerce.orders (store_id, payment_provider_account_id);

CREATE FUNCTION commerce.validate_payment_provider_account()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    account_capability TEXT;
BEGIN
    SELECT account.capability::text
      INTO account_capability
      FROM integration.provider_accounts AS account
     WHERE account.store_id = NEW.store_id
       AND account.id = NEW.payment_provider_account_id;

    IF account_capability IS DISTINCT FROM 'payment' THEN
        RAISE EXCEPTION 'payment_provider_account_id must reference a payment account'
            USING ERRCODE = '23503';
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION commerce.validate_shipping_provider_account()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    account_capability TEXT;
BEGIN
    SELECT account.capability::text
      INTO account_capability
      FROM integration.provider_accounts AS account
     WHERE account.store_id = NEW.store_id
       AND account.id = NEW.provider_account_id;

    IF account_capability IS DISTINCT FROM 'shipping' THEN
        RAISE EXCEPTION 'provider_account_id must reference a shipping account'
            USING ERRCODE = '23503';
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION commerce.prevent_order_identity_change()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.order_number IS DISTINCT FROM OLD.order_number
       OR NEW.store_id IS DISTINCT FROM OLD.store_id
       OR NEW.channel_id IS DISTINCT FROM OLD.channel_id
       OR NEW.shopper_id IS DISTINCT FROM OLD.shopper_id
       OR NEW.cart_id IS DISTINCT FROM OLD.cart_id
       OR NEW.price_list_id IS DISTINCT FROM OLD.price_list_id
       OR NEW.currency IS DISTINCT FROM OLD.currency
       OR NEW.payment_provider_account_id IS DISTINCT FROM OLD.payment_provider_account_id THEN
        RAISE EXCEPTION 'Order identity and payment provider binding are immutable after creation'
            USING ERRCODE = '22023';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER orders_payment_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, payment_provider_account_id
    ON commerce.orders
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_payment_provider_account();

CREATE TRIGGER orders_identity_immutable
    BEFORE UPDATE OF id, order_number, store_id, channel_id, shopper_id, cart_id,
        price_list_id, currency, payment_provider_account_id
    ON commerce.orders
    FOR EACH ROW EXECUTE FUNCTION commerce.prevent_order_identity_change();

CREATE TRIGGER refunds_payment_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, payment_provider_account_id
    ON commerce.order_refunds
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_payment_provider_account();

CREATE TRIGGER fulfillments_shipping_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, provider_account_id
    ON commerce.order_fulfillments
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_shipping_provider_account();

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_refunds ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_fulfillments ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.carts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.cart_lines
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.orders
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_lines
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_refunds
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_fulfillments
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.carts,
       commerce.cart_lines,
       commerce.orders,
       commerce.order_lines,
       commerce.order_refunds,
       commerce.order_fulfillments
    TO chaos_runtime;

REVOKE DELETE, TRUNCATE ON commerce.carts,
    commerce.orders,
    commerce.order_refunds,
    commerce.order_fulfillments
    FROM chaos_runtime;
REVOKE UPDATE ON commerce.orders FROM chaos_runtime;
GRANT UPDATE (
    payment_status,
    payment_provider_reference_id,
    payment_failure_code,
    fulfillment_status,
    refunded_amount_minor,
    subtotal_amount_minor,
    discount_amount_minor,
    tax_amount_minor,
    shipping_amount_minor,
    total_amount_minor,
    amounts_finalized_at,
    contact_email,
    contact_phone,
    billing_full_name,
    billing_address_line1,
    billing_address_line2,
    billing_locality,
    billing_administrative_area,
    billing_postal_code,
    billing_country_code,
    shipping_full_name,
    shipping_address_line1,
    shipping_address_line2,
    shipping_locality,
    shipping_administrative_area,
    shipping_postal_code,
    shipping_country_code,
    status,
    updated_at
)
    ON commerce.orders TO chaos_runtime;
REVOKE UPDATE, DELETE, TRUNCATE ON commerce.order_lines FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

REVOKE ALL ON FUNCTION commerce.validate_payment_provider_account() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.validate_shipping_provider_account() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.prevent_order_identity_change() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION commerce.validate_payment_provider_account() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.validate_shipping_provider_account() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.prevent_order_identity_change() TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce GRANT SELECT, INSERT ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA commerce GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
