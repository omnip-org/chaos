CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');
CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_payment_status AS ENUM ('pending', 'paid', 'failed', 'partially_refunded', 'refunded');
CREATE TYPE commerce.order_shipping_status AS ENUM ('pending', 'awaiting_pickup', 'shipped', 'delivered', 'cancelled');
CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');
CREATE TYPE commerce.fulfillment_status AS ENUM ('awaiting_pickup', 'shipped', 'delivered', 'cancelled');

CREATE TABLE commerce.carts (
    id               UUID                    NOT NULL PRIMARY KEY,
    store_id         UUID                    NOT NULL,
    sales_channel_id UUID                    NOT NULL,
    shopper_id       UUID                    NOT NULL,
    price_list_id    UUID                    NOT NULL,
    status           commerce.cart_status    NOT NULL DEFAULT 'active',
    version          BIGINT                  NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT carts_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT carts_store_id_id_shopper_id_key        UNIQUE (store_id, id, shopper_id),
    CONSTRAINT carts_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores (id),
    CONSTRAINT carts_store_id_sales_channel_fkey       FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.store_sales_channels (store_id, id),
    CONSTRAINT carts_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers (store_id, id),
    CONSTRAINT carts_store_id_price_list_fkey          FOREIGN KEY (store_id, price_list_id) REFERENCES commerce.price_lists (store_id, id),
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
    CONSTRAINT cart_lines_store_id_cart_fkey            FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts (store_id, id) ON DELETE CASCADE,
    CONSTRAINT cart_lines_store_id_product_variant_fkey FOREIGN KEY (store_id, product_id, product_variant_id) REFERENCES commerce.product_variants (store_id, product_id, id),
    CONSTRAINT cart_lines_product_title_length_check    CHECK (length(trim(product_title)) BETWEEN 1 AND 255),
    CONSTRAINT cart_lines_variant_title_length_check    CHECK (length(trim(variant_title)) BETWEEN 1 AND 255),
    CONSTRAINT cart_lines_sku_length_check              CHECK (sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64),
    CONSTRAINT cart_lines_quantity_range_check          CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT cart_lines_unit_price_nonnegative_check  CHECK (unit_price_amount_minor >= 0)
);

CREATE TABLE commerce.orders (
    id                              UUID                               NOT NULL PRIMARY KEY,
    store_id                        UUID                               NOT NULL,
    order_number                    TEXT                               NOT NULL,
    sales_channel_id                UUID                               NOT NULL,
    cart_id                         UUID                               NOT NULL,
    shopper_id                      UUID                               NOT NULL,
    idempotency_key                 UUID                               NOT NULL,
    checkout_request_fingerprint    BYTEA,
    price_list_id                   UUID                               NOT NULL,
    currency                        CHAR(3)                            NOT NULL,
    status                          commerce.order_status              NOT NULL DEFAULT 'pending',
    payment_status                  commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    payment_provider_account_id     UUID                               NOT NULL,
    payment_provider_reference_id   TEXT,
    payment_failure_code            TEXT,
    shipping_status                 commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    shipping_provider_account_id    UUID,
    shipping_provider_reference_id  TEXT,
    refunded_amount_minor           BIGINT                             NOT NULL DEFAULT 0,
    subtotal_amount_minor           BIGINT                             NOT NULL,
    discount_amount_minor           BIGINT                             NOT NULL,
    tax_amount_minor                BIGINT                             NOT NULL,
    shipping_amount_minor           BIGINT                             NOT NULL,
    total_amount_minor              BIGINT                             NOT NULL,
    contact_email                   extensions.citext,
    contact_phone                   TEXT,
    billing_full_name               TEXT,
    billing_company                 TEXT,
    billing_address_line1           TEXT,
    billing_address_line2           TEXT,
    billing_locality                TEXT,
    billing_administrative_area     TEXT,
    billing_postal_code             TEXT,
    billing_country_code            CHAR(2),
    shipping_full_name              TEXT,
    shipping_company                TEXT,
    shipping_address_line1          TEXT,
    shipping_address_line2          TEXT,
    shipping_locality               TEXT,
    shipping_administrative_area    TEXT,
    shipping_postal_code            TEXT,
    shipping_country_code           CHAR(2),
    created_at                      TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT orders_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT orders_store_id_order_number_key         UNIQUE (store_id, order_number),
    CONSTRAINT orders_store_id_id_currency_key          UNIQUE (store_id, id, currency),
    CONSTRAINT orders_store_id_id_shopper_id_key        UNIQUE (store_id, id, shopper_id),
    CONSTRAINT orders_store_id_sales_channel_id_shopper_id_idempotency_key_key UNIQUE (store_id, sales_channel_id, shopper_id, idempotency_key),
    CONSTRAINT orders_store_id_cart_fkey                FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts (store_id, id),
    CONSTRAINT orders_store_id_shopper_fkey             FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers (store_id, id),
    CONSTRAINT orders_store_id_sales_channel_fkey       FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.store_sales_channels (store_id, id),
    CONSTRAINT orders_store_id_price_list_currency_fkey FOREIGN KEY (store_id, price_list_id, currency) REFERENCES commerce.price_lists (store_id, id, currency),
    CONSTRAINT orders_store_id_payment_provider_account_fkey FOREIGN KEY (store_id, payment_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT orders_store_id_shipping_provider_account_fkey FOREIGN KEY (store_id, shipping_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT orders_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_idempotency_key_not_nil_check     CHECK (idempotency_key <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT orders_checkout_request_fingerprint_check CHECK (checkout_request_fingerprint IS NULL OR octet_length(checkout_request_fingerprint) = 32),
    CONSTRAINT orders_order_number_check                CHECK (order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'),
    CONSTRAINT orders_amounts_check                     CHECK (subtotal_amount_minor >= 0 AND discount_amount_minor >= 0 AND tax_amount_minor >= 0 AND shipping_amount_minor >= 0 AND total_amount_minor >= 0 AND refunded_amount_minor >= 0 AND refunded_amount_minor <= total_amount_minor),
    CONSTRAINT orders_contact_email_length_check        CHECK (contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320),
    CONSTRAINT orders_contact_phone_format_check        CHECK (contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'),
    CONSTRAINT orders_billing_country_code_check        CHECK (billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_shipping_country_code_check       CHECK (shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_payment_provider_reference_check  CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT orders_shipping_provider_reference_check CHECK (shipping_provider_reference_id IS NULL OR (shipping_provider_account_id IS NOT NULL AND length(trim(shipping_provider_reference_id)) BETWEEN 1 AND 255)),
    CONSTRAINT orders_payment_failure_code_check        CHECK (payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000)
);

CREATE TABLE commerce.order_tracking_tokens (
    store_id     UUID        NOT NULL,
    order_id     UUID        NOT NULL,
    token_digest BYTEA       NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT order_tracking_tokens_pkey                PRIMARY KEY (store_id, order_id),
    CONSTRAINT order_tracking_tokens_store_id_token_key  UNIQUE (store_id, token_digest),
    CONSTRAINT order_tracking_tokens_store_id_order_fkey FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_tokens_digest_check        CHECK (octet_length(token_digest) = 32),
    CONSTRAINT order_tracking_tokens_expiry_check        CHECK (expires_at > created_at)
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
    created_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT order_lines_pkey                          PRIMARY KEY (store_id, order_id, position),
    CONSTRAINT order_lines_store_id_order_id_variant_key UNIQUE (store_id, order_id, product_variant_id),
    CONSTRAINT order_lines_store_id_order_fkey           FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id),
    CONSTRAINT order_lines_position_check                CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT order_lines_product_title_length_check    CHECK (length(trim(product_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_variant_title_length_check    CHECK (length(trim(variant_title)) BETWEEN 1 AND 255),
    CONSTRAINT order_lines_sku_length_check              CHECK (sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64),
    CONSTRAINT order_lines_quantity_range_check          CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT order_lines_amounts_check                 CHECK (unit_price_amount_minor >= 0 AND subtotal_amount_minor = unit_price_amount_minor * quantity AND subtotal_amount_minor >= 0)
);

CREATE TABLE commerce.refunds (
    id                            UUID                   NOT NULL PRIMARY KEY,
    store_id                      UUID                   NOT NULL,
    order_id                      UUID                   NOT NULL,
    currency                      CHAR(3)                NOT NULL,
    status                        commerce.refund_status NOT NULL DEFAULT 'pending',
    amount_minor                  BIGINT                 NOT NULL,
    payment_provider_account_id   UUID                   NOT NULL,
    payment_provider_reference_id TEXT,
    failure_code                  TEXT,
    created_at                    TIMESTAMPTZ            NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ            NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT refunds_store_id_id_key                  UNIQUE (store_id, id),
    CONSTRAINT refunds_store_id_order_currency_fkey     FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders (store_id, id, currency),
    CONSTRAINT refunds_store_id_payment_provider_account_fkey FOREIGN KEY (store_id, payment_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT refunds_amount_positive_check            CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check            CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_payment_provider_reference_check CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT refunds_failure_code_check               CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_failure_code_shape_check         CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE TABLE commerce.fulfillments (
    id                           UUID                        NOT NULL PRIMARY KEY,
    store_id                     UUID                        NOT NULL,
    order_id                     UUID                        NOT NULL,
    shipping_provider_account_id UUID                        NOT NULL,
    status                       commerce.fulfillment_status NOT NULL DEFAULT 'awaiting_pickup',
    tracking_number              TEXT,
    tracking_url                 TEXT,
    shipped_at                   TIMESTAMPTZ,
    delivered_at                 TIMESTAMPTZ,
    cancelled_at                 TIMESTAMPTZ,
    created_at                   TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT fulfillments_store_id_id_key                    UNIQUE (store_id, id),
    CONSTRAINT fulfillments_store_id_order_fkey                FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id),
    CONSTRAINT fulfillments_store_id_shipping_provider_account_fkey FOREIGN KEY (store_id, shipping_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT fulfillments_tracking_number_check              CHECK (tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255),
    CONSTRAINT fulfillments_tracking_url_check                 CHECK (tracking_url IS NULL OR (length(tracking_url) BETWEEN 9 AND 2048 AND tracking_url ~ '^https://')),
    CONSTRAINT fulfillments_shape_check                        CHECK (
        (status = 'awaiting_pickup' AND shipped_at IS NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'shipped' AND shipped_at IS NOT NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'delivered' AND shipped_at IS NOT NULL AND delivered_at IS NOT NULL AND cancelled_at IS NULL) OR
        (status = 'cancelled' AND cancelled_at IS NOT NULL)
    )
);

CREATE INDEX carts_channel_updated_idx ON commerce.carts (store_id, sales_channel_id, status, updated_at DESC, id DESC);
CREATE INDEX carts_store_shopper_idx ON commerce.carts (store_id, shopper_id, id);
CREATE INDEX carts_store_price_list_idx ON commerce.carts (store_id, price_list_id, id);
CREATE INDEX cart_lines_variant_lookup_idx ON commerce.cart_lines (store_id, product_variant_id, cart_id);
CREATE INDEX cart_lines_product_variant_fk_idx ON commerce.cart_lines (store_id, product_id, product_variant_id, cart_id);
CREATE INDEX orders_channel_created_idx ON commerce.orders (store_id, sales_channel_id, created_at DESC, id DESC);
CREATE INDEX orders_store_status_id_idx ON commerce.orders (store_id, status, id DESC);
CREATE INDEX orders_store_contact_email_id_idx ON commerce.orders (store_id, contact_email, id DESC) WHERE contact_email IS NOT NULL;
CREATE INDEX orders_store_cart_idx ON commerce.orders (store_id, cart_id);
CREATE INDEX orders_store_shopper_idx ON commerce.orders (store_id, shopper_id);
CREATE INDEX orders_store_price_list_currency_idx ON commerce.orders (store_id, price_list_id, currency);
CREATE INDEX order_tracking_tokens_expiry_idx ON commerce.order_tracking_tokens (expires_at, store_id, order_id);
CREATE UNIQUE INDEX orders_one_pending_per_cart_idx ON commerce.orders (store_id, cart_id) WHERE status = 'pending';
CREATE UNIQUE INDEX orders_payment_provider_reference_key ON commerce.orders (store_id, payment_provider_account_id, payment_provider_reference_id) WHERE payment_provider_reference_id IS NOT NULL;
CREATE UNIQUE INDEX orders_shipping_provider_reference_key ON commerce.orders (store_id, shipping_provider_account_id, shipping_provider_reference_id) WHERE shipping_provider_reference_id IS NOT NULL;
CREATE INDEX refunds_order_created_idx ON commerce.refunds (store_id, order_id, created_at DESC);
CREATE INDEX refunds_payment_provider_account_idx ON commerce.refunds (store_id, payment_provider_account_id, order_id);
CREATE UNIQUE INDEX refunds_payment_provider_reference_key ON commerce.refunds (store_id, payment_provider_account_id, payment_provider_reference_id) WHERE payment_provider_reference_id IS NOT NULL;
CREATE INDEX fulfillments_order_created_idx ON commerce.fulfillments (store_id, order_id, created_at DESC);
CREATE INDEX fulfillments_shipping_provider_account_idx ON commerce.fulfillments (store_id, shipping_provider_account_id, order_id);
CREATE INDEX orders_payment_provider_account_idx ON commerce.orders (store_id, payment_provider_account_id);
CREATE INDEX orders_shipping_provider_account_idx ON commerce.orders (store_id, shipping_provider_account_id) WHERE shipping_provider_account_id IS NOT NULL;

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
    IF NEW.shipping_provider_account_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT account.capability::text
      INTO account_capability
      FROM integration.provider_accounts AS account
     WHERE account.store_id = NEW.store_id
       AND account.id = NEW.shipping_provider_account_id;

    IF account_capability IS DISTINCT FROM 'shipping' THEN
        RAISE EXCEPTION 'shipping_provider_account_id must reference a shipping account'
            USING ERRCODE = '23503';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER orders_payment_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, payment_provider_account_id
    ON commerce.orders
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_payment_provider_account();

CREATE TRIGGER orders_shipping_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, shipping_provider_account_id
    ON commerce.orders
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_shipping_provider_account();

CREATE TRIGGER refunds_payment_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, payment_provider_account_id
    ON commerce.refunds
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_payment_provider_account();

CREATE TRIGGER fulfillments_shipping_provider_capability_check
    BEFORE INSERT OR UPDATE OF store_id, shipping_provider_account_id
    ON commerce.fulfillments
    FOR EACH ROW EXECUTE FUNCTION commerce.validate_shipping_provider_account();

CREATE FUNCTION commerce.cleanup_expired_order_tracking_tokens(batch_size INTEGER)
RETURNS INTEGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    IF batch_size IS NULL OR batch_size NOT BETWEEN 1 AND 10000 THEN
        RAISE EXCEPTION 'batch_size must be between 1 and 10000'
            USING ERRCODE = '22023';
    END IF;

    WITH candidates AS (
        SELECT store_id, order_id
        FROM commerce.order_tracking_tokens
        WHERE expires_at < CURRENT_TIMESTAMP
        ORDER BY expires_at, store_id, order_id
        LIMIT batch_size
    )
    DELETE FROM commerce.order_tracking_tokens AS token
     USING candidates
     WHERE token.store_id = candidates.store_id
       AND token.order_id = candidates.order_id;

    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END
$$;

CREATE FUNCTION integration.set_provider_webhook_aggregate (
    event_id           UUID,
    resolved_type      TEXT,
    resolved_aggregate UUID
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH updated AS (
        UPDATE integration.provider_webhook_inbox AS event
           SET aggregate_type = resolved_type,
               aggregate_id = resolved_aggregate
         WHERE event.id = event_id
           AND resolved_type = 'order'
           AND event.processing_status = 'pending'
           AND event.store_id = nullif(current_setting('app.store_id', true), '')::uuid
           AND EXISTS (
               SELECT 1
               FROM commerce.orders AS order_row
               WHERE order_row.store_id = event.store_id
                 AND order_row.id = resolved_aggregate
           )
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM updated);
$$;

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_tracking_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.fulfillments ENABLE ROW LEVEL SECURITY;

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

CREATE POLICY store_isolation ON commerce.refunds
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.fulfillments
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.carts,
       commerce.cart_lines,
       commerce.orders,
       commerce.order_tracking_tokens,
       commerce.order_lines,
       commerce.refunds,
       commerce.fulfillments
    TO chaos_runtime;

REVOKE DELETE ON commerce.orders FROM chaos_runtime;
REVOKE UPDATE, DELETE ON commerce.order_lines FROM chaos_runtime;
REVOKE DELETE ON commerce.refunds FROM chaos_runtime;
REVOKE DELETE ON commerce.fulfillments FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

REVOKE ALL ON FUNCTION commerce.validate_payment_provider_account() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.validate_shipping_provider_account() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.cleanup_expired_order_tracking_tokens(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.set_provider_webhook_aggregate (UUID, TEXT, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION commerce.validate_payment_provider_account() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.validate_shipping_provider_account() TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.cleanup_expired_order_tracking_tokens(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.set_provider_webhook_aggregate (UUID, TEXT, UUID) TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
