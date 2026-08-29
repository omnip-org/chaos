CREATE TYPE commerce.checkout_attempt_status AS ENUM (
    'creating',
    'open',
    'paid',
    'failed',
    'cancelled',
    'expired'
);

CREATE TABLE commerce.checkout_attempts (
    id                           UUID                              NOT NULL PRIMARY KEY,
    store_id                    UUID                              NOT NULL,
    sales_channel_id            UUID                              NOT NULL,
    shopper_id                  UUID                              NOT NULL,
    source_cart_id              UUID                              NOT NULL,
    successor_cart_id           UUID                              NOT NULL,
    order_id                    UUID                              NOT NULL,
    status                      commerce.checkout_attempt_status  NOT NULL DEFAULT 'creating',
    payment_provider_account_id UUID                              NOT NULL,
    client_idempotency_key      UUID                              NOT NULL,
    provider_idempotency_key    UUID                              NOT NULL,
    provider_session_id         TEXT,
    provider_public_key         TEXT,
    provider_client_secret      TEXT,
    return_url                  TEXT                              NOT NULL,
    request_fingerprint         BYTEA                             NOT NULL,
    shipping_policy_version     BIGINT                            NOT NULL,
    shipping_countries_snapshot JSONB                             NOT NULL,
    expires_at                  TIMESTAMPTZ                       NOT NULL,
    created_at                  TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                  TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT checkout_attempts_store_id_id_key
        UNIQUE (store_id, id),
    CONSTRAINT checkout_attempts_store_id_source_cart_key
        UNIQUE (store_id, source_cart_id),
    CONSTRAINT checkout_attempts_store_id_successor_cart_key
        UNIQUE (store_id, successor_cart_id),
    CONSTRAINT checkout_attempts_store_id_order_key
        UNIQUE (store_id, order_id),
    CONSTRAINT checkout_attempts_store_channel_shopper_idempotency_key
        UNIQUE (store_id, sales_channel_id, shopper_id, client_idempotency_key),
    CONSTRAINT checkout_attempts_store_id_source_cart_fkey
        FOREIGN KEY (store_id, source_cart_id) REFERENCES commerce.carts (store_id, id),
    CONSTRAINT checkout_attempts_store_id_successor_cart_fkey
        FOREIGN KEY (store_id, successor_cart_id) REFERENCES commerce.carts (store_id, id),
    CONSTRAINT checkout_attempts_store_id_order_fkey
        FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders (store_id, id),
    CONSTRAINT checkout_attempts_store_id_shopper_fkey
        FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers (store_id, id),
    CONSTRAINT checkout_attempts_store_id_channel_fkey
        FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.store_sales_channels (store_id, id),
    CONSTRAINT checkout_attempts_store_id_provider_account_fkey
        FOREIGN KEY (store_id, payment_provider_account_id) REFERENCES integration.provider_accounts (store_id, id),
    CONSTRAINT checkout_attempts_client_idempotency_key_check
        CHECK (client_idempotency_key <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT checkout_attempts_provider_idempotency_key_check
        CHECK (provider_idempotency_key <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT checkout_attempts_provider_session_id_check
        CHECK (provider_session_id IS NULL OR length(trim(provider_session_id)) BETWEEN 1 AND 255),
    CONSTRAINT checkout_attempts_provider_public_key_check
        CHECK (provider_public_key IS NULL OR length(trim(provider_public_key)) BETWEEN 1 AND 255),
    CONSTRAINT checkout_attempts_provider_client_secret_check
        CHECK (provider_client_secret IS NULL OR length(trim(provider_client_secret)) BETWEEN 1 AND 2048),
    CONSTRAINT checkout_attempts_return_url_check
        CHECK (length(trim(return_url)) BETWEEN 10 AND 2048),
    CONSTRAINT checkout_attempts_request_fingerprint_check
        CHECK (octet_length(request_fingerprint) = 32),
    CONSTRAINT checkout_attempts_shipping_policy_version_check
        CHECK (shipping_policy_version >= 1),
    CONSTRAINT checkout_attempts_shipping_countries_snapshot_check
        CHECK (jsonb_typeof(shipping_countries_snapshot) = 'array' AND pg_column_size(shipping_countries_snapshot) <= 8192),
    CONSTRAINT checkout_attempts_expiry_check
        CHECK (expires_at > created_at)
);

CREATE UNIQUE INDEX checkout_attempts_provider_session_key
    ON commerce.checkout_attempts (store_id, payment_provider_account_id, provider_session_id)
    WHERE provider_session_id IS NOT NULL;
CREATE INDEX checkout_attempts_store_shopper_status_idx
    ON commerce.checkout_attempts (store_id, sales_channel_id, shopper_id, status, created_at DESC, id DESC);
CREATE INDEX checkout_attempts_expiry_idx
    ON commerce.checkout_attempts (expires_at, store_id, id)
    WHERE status IN ('creating', 'open');

CREATE FUNCTION commerce.expire_checkout_attempts(batch_size INTEGER)
RETURNS INTEGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    candidate RECORD;
    expired_count INTEGER := 0;
BEGIN
    IF batch_size IS NULL OR batch_size NOT BETWEEN 1 AND 10000 THEN
        RAISE EXCEPTION 'batch_size must be between 1 and 10000'
            USING ERRCODE = '22023';
    END IF;

    FOR candidate IN
        SELECT attempt.store_id, attempt.id, attempt.order_id, attempt.source_cart_id
        FROM commerce.checkout_attempts AS attempt
        INNER JOIN commerce.orders AS sales_order
            ON sales_order.store_id = attempt.store_id
           AND sales_order.id = attempt.order_id
        WHERE attempt.status IN ('creating', 'open')
          AND attempt.expires_at <= CURRENT_TIMESTAMP
          AND sales_order.status = 'pending'
        ORDER BY attempt.expires_at, attempt.store_id, attempt.id
        LIMIT batch_size
        FOR UPDATE OF attempt, sales_order SKIP LOCKED
    LOOP
        UPDATE commerce.product_variants AS variant
           SET reserved_quantity = variant.reserved_quantity - LEAST(variant.reserved_quantity, lines.quantity),
               updated_at = CURRENT_TIMESTAMP
          FROM (
              SELECT product_variant_id, SUM(quantity)::bigint AS quantity
              FROM commerce.order_lines
              WHERE store_id = candidate.store_id
                AND order_id = candidate.order_id
                AND track_inventory
              GROUP BY product_variant_id
          ) AS lines
         WHERE variant.store_id = candidate.store_id
           AND variant.id = lines.product_variant_id;

        UPDATE commerce.orders
           SET status = 'cancelled',
               payment_status = 'failed',
               payment_failure_code = 'checkout_expired',
               updated_at = CURRENT_TIMESTAMP
         WHERE store_id = candidate.store_id
           AND id = candidate.order_id
           AND status = 'pending';

        UPDATE commerce.carts
           SET status = 'abandoned',
               version = version + 1,
               updated_at = CURRENT_TIMESTAMP
         WHERE store_id = candidate.store_id
           AND id = candidate.source_cart_id
           AND status = 'checkout_pending';

        UPDATE commerce.checkout_attempts
           SET status = 'expired', updated_at = CURRENT_TIMESTAMP
         WHERE store_id = candidate.store_id
           AND id = candidate.id
           AND status IN ('creating', 'open');

        expired_count := expired_count + 1;
    END LOOP;

    RETURN expired_count;
END
$$;

ALTER TABLE commerce.checkout_attempts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.checkout_attempts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE
    ON commerce.checkout_attempts TO chaos_runtime;

REVOKE ALL ON FUNCTION commerce.expire_checkout_attempts(INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION commerce.expire_checkout_attempts(INTEGER) TO chaos_runtime;
