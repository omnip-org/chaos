CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');
CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');
CREATE TYPE commerce.order_payment_status AS ENUM ('pending', 'paid', 'failed', 'partially_refunded', 'refunded');
CREATE TYPE commerce.order_shipping_status AS ENUM ('pending', 'awaiting_pickup', 'shipped', 'delivered', 'cancelled');

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
    payment_status               commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    payment_provider_account_id UUID                               NOT NULL,
    payment_provider_reference_id TEXT,
    payment_failure_code         TEXT,
    shipping_status              commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    shipping_provider_account_id UUID,
    shipping_provider_reference_id TEXT,
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
    CONSTRAINT orders_payment_provider_account_fkey     FOREIGN KEY (payment_provider_account_id) REFERENCES integration.payment_provider_accounts(id),
    CONSTRAINT orders_shipping_provider_account_fkey    FOREIGN KEY (shipping_provider_account_id) REFERENCES integration.shipping_provider_accounts(id),
    CONSTRAINT orders_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_request_id_not_nil_check           CHECK (request_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT orders_order_number_check                CHECK (order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'),
    CONSTRAINT orders_amounts_check                     CHECK (subtotal_amount_minor >= 0 AND discount_amount_minor >= 0 AND tax_amount_minor >= 0 AND shipping_amount_minor >= 0 AND total_amount_minor >= 0 AND refunded_amount_minor >= 0 AND refunded_amount_minor <= total_amount_minor),
    CONSTRAINT orders_contact_email_length_check        CHECK (contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320),
    CONSTRAINT orders_contact_phone_format_check        CHECK (contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'),
    CONSTRAINT orders_billing_country_code_check        CHECK (billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_shipping_country_code_check       CHECK (shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT orders_payment_provider_reference_check CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT orders_shipping_provider_reference_check CHECK (shipping_provider_reference_id IS NULL OR (shipping_provider_account_id IS NOT NULL AND length(trim(shipping_provider_reference_id)) BETWEEN 1 AND 255)),
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

ALTER TABLE commerce.orders ADD UNIQUE (store_id, id, currency);

CREATE INDEX carts_channel_updated_idx ON commerce.carts (store_id, sales_channel_id, status, updated_at DESC, id DESC);
CREATE INDEX cart_lines_variant_lookup_idx ON commerce.cart_lines (store_id, product_variant_id, cart_id);
CREATE INDEX orders_channel_created_idx ON commerce.orders (store_id, sales_channel_id, created_at DESC, id DESC);
CREATE INDEX order_tracking_tokens_expiry_idx ON commerce.order_tracking_tokens (expires_at, store_id, order_id);
CREATE UNIQUE INDEX orders_payment_provider_reference_key
    ON commerce.orders (store_id, payment_provider_account_id, payment_provider_reference_id)
    WHERE payment_provider_reference_id IS NOT NULL;
CREATE UNIQUE INDEX orders_shipping_provider_reference_key
    ON commerce.orders (store_id, shipping_provider_account_id, shipping_provider_reference_id)
    WHERE shipping_provider_reference_id IS NOT NULL;

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_tracking_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;

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

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.carts,
       commerce.cart_lines,
       commerce.orders,
       commerce.order_tracking_tokens,
       commerce.order_lines
    TO chaos_runtime;

REVOKE DELETE ON commerce.orders FROM chaos_runtime;
REVOKE UPDATE, DELETE ON commerce.order_lines FROM chaos_runtime;

SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_webhooks');

CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');

CREATE TABLE commerce.refunds (
    id                     UUID                     NOT NULL PRIMARY KEY,
    store_id               UUID                     NOT NULL,
    order_id               UUID                     NOT NULL,
    currency               CHAR(3)                  NOT NULL,
    status                 commerce.refund_status   NOT NULL DEFAULT 'pending',
    amount_minor           BIGINT                   NOT NULL,
    payment_provider_account_id UUID                     NOT NULL,
    payment_provider_reference_id TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT refunds_store_id_id_key                  UNIQUE (store_id, id),
    CONSTRAINT refunds_store_id_order_currency_fkey      FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders(store_id, id, currency),
    CONSTRAINT refunds_payment_provider_account_fkey     FOREIGN KEY (payment_provider_account_id) REFERENCES integration.payment_provider_accounts(id),
    CONSTRAINT refunds_amount_positive_check             CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_payment_provider_reference_check CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT refunds_failure_code_check                CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_failure_code_shape_check          CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE INDEX refunds_order_created_idx ON commerce.refunds (store_id, order_id, created_at DESC);
CREATE UNIQUE INDEX refunds_payment_provider_reference_key
    ON commerce.refunds (store_id, payment_provider_account_id, payment_provider_reference_id)
    WHERE payment_provider_reference_id IS NOT NULL;

CREATE FUNCTION commerce.claim_event_outbox(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
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
    SELECT event.id, event.store_id, event.event_type, event.payload, event.attempts
      FROM integration.claim_routed_event_outbox(
               'chaos_payment_commands', batch_size
           ) AS event;
$$;

CREATE FUNCTION commerce.resolve_provider_account(
    requested_provider             integration.payment_provider,
    requested_provider_account_id  UUID
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
     FROM integration.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider             integration.payment_provider,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    secret_reference    TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.webhook_secret_reference
      FROM integration.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.webhook_secret_reference IS NOT NULL;
$$;

CREATE TABLE commerce.provider_webhooks (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    provider_account_id  UUID        NOT NULL,
    provider_event_id    TEXT        NOT NULL,
    event_type           TEXT        NOT NULL,
    payload              JSONB       NOT NULL,
    order_id             UUID,
    pgmq_message_id      BIGINT      NOT NULL UNIQUE,
    processed_at         TIMESTAMPTZ,
    failed_at            TIMESTAMPTZ,
    last_error           TEXT,
    verified_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT provider_webhooks_provider_account_id_provider_event_id_key    UNIQUE (provider_account_id, provider_event_id),
    CONSTRAINT provider_webhooks_store_id_fkey                                FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT provider_webhooks_provider_account_fkey                         FOREIGN KEY (provider_account_id) REFERENCES integration.payment_provider_accounts(id),
    CONSTRAINT provider_webhooks_store_id_order_fkey                          FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    CONSTRAINT provider_webhooks_payload_object_check                         CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT provider_webhooks_completion_check                             CHECK (processed_at IS NULL OR failed_at IS NULL)
);

CREATE INDEX provider_webhooks_claim_idx ON commerce.provider_webhooks (created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;
CREATE INDEX provider_webhooks_order_idx ON commerce.provider_webhooks (store_id, order_id, created_at DESC) WHERE order_id IS NOT NULL;
CREATE FUNCTION commerce.enqueue_webhook_event()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          'chaos_webhooks',
          jsonb_build_object('version', 1, 'webhook_event_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.claim_webhook_events(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    message RECORD;
    target RECORD;
BEGIN
    FOR message IN
        SELECT queued.msg_id, queued.read_ct
          FROM pgmq.read(
                   'chaos_webhooks',
                   120,
                   greatest(least(batch_size, 100), 1),
                   '{}'::jsonb
               ) AS queued
    LOOP
        SELECT event.id,
               event.store_id,
               event.event_type,
               event.payload
          INTO target
          FROM commerce.provider_webhooks AS event
         WHERE event.pgmq_message_id = message.msg_id
           AND event.processed_at IS NULL
           AND event.failed_at IS NULL;
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_webhooks', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        event_type := target.event_type;
        payload := target.payload;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION commerce.finish_webhook_event(
    event_id UUID,
    attempts INTEGER,
    succeeded BOOLEAN,
    failure TEXT,
    max_attempts INTEGER,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    message_id BIGINT;
BEGIN
    SELECT event.pgmq_message_id
      INTO message_id
      FROM commerce.provider_webhooks AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE commerce.provider_webhooks AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSE
        UPDATE commerce.provider_webhooks AS event
           SET last_error = left(failure, 2000)
         WHERE event.id = event_id;
        PERFORM pgmq.set_vt(
            'chaos_webhooks',
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

CREATE FUNCTION commerce.set_webhook_order_id(
    event_id UUID,
    resolved_order_id UUID
)
RETURNS BOOLEAN
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    UPDATE commerce.provider_webhooks
       SET order_id = resolved_order_id
     WHERE id = event_id;
    RETURN FOUND;
END;
$$;

CREATE INDEX provider_webhooks_provider_account_idx ON commerce.provider_webhooks (provider_account_id, created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;

ALTER TABLE commerce.provider_webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.provider_webhooks
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.refunds
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE TRIGGER provider_webhooks_enqueue
    BEFORE INSERT ON commerce.provider_webhooks
    FOR EACH ROW
    EXECUTE FUNCTION commerce.enqueue_webhook_event();

INSERT INTO integration.event_consumers (event_type, queue_name, description)
VALUES
    ('refund.create_requested', 'chaos_payment_commands', 'Creates a Stripe Refund for the Order');

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(integration.payment_provider, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_event_outbox(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(integration.payment_provider, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.enqueue_webhook_event() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_webhook_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(integration.payment_provider, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_event_outbox(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_provider_webhook_secret_references(integration.payment_provider, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_webhook_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.provider_webhooks,
       commerce.refunds
    TO chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.provider_webhooks FROM chaos_runtime;
REVOKE DELETE ON commerce.refunds FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;

CREATE TYPE commerce.fulfillment_status AS ENUM ('awaiting_pickup', 'shipped', 'delivered', 'cancelled');

CREATE TABLE commerce.fulfillments (
    id                              UUID                          NOT NULL PRIMARY KEY,
    store_id                        UUID                          NOT NULL,
    order_id                        UUID                          NOT NULL,
    shipping_provider_account_id    UUID                          NOT NULL,
    status                          commerce.fulfillment_status   NOT NULL DEFAULT 'awaiting_pickup',
    tracking_number                 TEXT,
    tracking_url                    TEXT,
    shipped_at                      TIMESTAMPTZ,
    delivered_at                    TIMESTAMPTZ,
    cancelled_at                    TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT fulfillments_store_id_id_key                    UNIQUE (store_id, id),
    CONSTRAINT fulfillments_store_id_order_fkey                FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    CONSTRAINT fulfillments_provider_account_fkey              FOREIGN KEY (shipping_provider_account_id) REFERENCES integration.shipping_provider_accounts(id),
    CONSTRAINT fulfillments_tracking_number_check              CHECK (tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255),
    CONSTRAINT fulfillments_tracking_url_check                 CHECK (tracking_url IS NULL OR (length(tracking_url) BETWEEN 9 AND 2048 AND tracking_url ~ '^https://')),
    CONSTRAINT fulfillments_shape_check                        CHECK (
        (status = 'awaiting_pickup' AND shipped_at IS NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'shipped' AND shipped_at IS NOT NULL AND delivered_at IS NULL AND cancelled_at IS NULL) OR
        (status = 'delivered' AND shipped_at IS NOT NULL AND delivered_at IS NOT NULL AND cancelled_at IS NULL) OR
        (status = 'cancelled' AND cancelled_at IS NOT NULL)
    )
);

CREATE INDEX fulfillments_order_created_idx ON commerce.fulfillments (store_id, order_id, created_at DESC);

ALTER TABLE commerce.fulfillments ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.fulfillments
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.fulfillments
    TO chaos_runtime;

REVOKE DELETE ON commerce.fulfillments FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
