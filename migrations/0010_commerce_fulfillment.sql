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
    CONSTRAINT fulfillments_store_id_provider_account_fkey     FOREIGN KEY (store_id, shipping_provider_account_id) REFERENCES integration.shipping_provider_accounts(store_id, id),
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
