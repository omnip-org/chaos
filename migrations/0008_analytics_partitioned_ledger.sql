-- === Analytics event ledger: identity links out, manual partitions in ===
--
-- The Analytics ledger is an append-only analysis log. It keeps the IDs that
-- are useful for reporting, but it does not own or constrain business rows.
-- Retention is deliberately an operator decision; this migration installs
-- partition maintenance only and does not configure a retention policy.

DROP FUNCTION IF EXISTS integration.purge_expired_analytics_data(
    INTEGER, TIMESTAMPTZ
);

ALTER TABLE integration.analytics_settings
    DROP CONSTRAINT IF EXISTS analytics_settings_retention_check;
ALTER TABLE integration.analytics_settings
    DROP COLUMN IF EXISTS identity_linking_enabled,
    DROP COLUMN IF EXISTS raw_event_retention_days;

DROP TABLE IF EXISTS integration.visitor_customer_links CASCADE;

-- Delivery state intentionally has no foreign key into the partitioned event
-- log. Before an operator removes a partition, its delivery rows must be
-- removed explicitly (see docs/analytics-operations.md).
ALTER TABLE integration.analytics_event_deliveries
    DROP CONSTRAINT IF EXISTS analytics_event_deliveries_store_id_commerce_event_id_fkey;
DROP INDEX IF EXISTS integration.analytics_event_deliveries_event_idx;

-- Keep the old table available until all rows have been copied into the new
-- partitioned parent. This is a one-time rewrite, so it is intentionally
-- explicit rather than relying on a background backfill that could split the
-- ledger during deployment.
ALTER TABLE integration.commerce_events
    RENAME TO commerce_events_legacy;
ALTER TABLE integration.commerce_events_legacy
    DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS integration.commerce_events_visitor_path_idx;
DROP INDEX IF EXISTS integration.commerce_events_customer_path_idx;
DROP INDEX IF EXISTS integration.commerce_events_channel_time_idx;
DROP INDEX IF EXISTS integration.commerce_events_retention_idx;

CREATE TABLE integration.commerce_events (
    id                          UUID                            NOT NULL,
    event_id                    UUID                            NOT NULL,
    store_id                    UUID                            NOT NULL,
    sales_channel_id            UUID                            NOT NULL,
    event_name                  integration.commerce_event_name NOT NULL,
    source                      integration.event_source         NOT NULL,
    collection_basis            integration.browser_collection_basis NOT NULL,
    schema_version              SMALLINT                        NOT NULL,
    visitor_id                  UUID,
    session_id                  UUID,
    customer_id                 UUID,
    product_id                  UUID,
    product_variant_id          UUID,
    cart_id                     UUID,
    checkout_id                 UUID,
    order_id                    UUID,
    payment_attempt_id          UUID,
    refund_id                   UUID,
    path                        TEXT,
    value_minor                 BIGINT,
    currency                    CHAR(3),
    analytics_storage_consent   BOOLEAN                         NOT NULL,
    advertising_storage_consent BOOLEAN                         NOT NULL,
    provider_eligible           BOOLEAN                         NOT NULL,
    consent_policy_version      TEXT,
    settings_revision            INTEGER                         NOT NULL,
    properties                  JSONB                            NOT NULL DEFAULT '{}'::jsonb,
    occurred_at                 TIMESTAMPTZ                      NOT NULL,
    received_at                 TIMESTAMPTZ                      NOT NULL,
    created_at                  TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- PostgreSQL requires the partition key in parent primary/unique keys.
    -- Global event-id deduplication is therefore guarded in the repository by
    -- a transaction-scoped advisory lock plus an existence check.
    CONSTRAINT commerce_events_received_id_pkey PRIMARY KEY (received_at, id),
    CONSTRAINT commerce_events_store_received_event_key
        UNIQUE (store_id, received_at, event_id),
    CONSTRAINT commerce_events_schema_version_check CHECK (schema_version = 1),
    CONSTRAINT commerce_events_identity_check CHECK (
        (visitor_id IS NULL OR visitor_id <> '00000000-0000-0000-0000-000000000000'::uuid)
        AND (session_id IS NULL OR session_id <> '00000000-0000-0000-0000-000000000000'::uuid)
    ),
    CONSTRAINT commerce_events_browser_shape_check CHECK (
        (source = 'browser' AND visitor_id IS NOT NULL AND session_id IS NOT NULL
            AND collection_basis IN ('consent', 'store_policy')
            AND (collection_basis = 'store_policy' OR analytics_storage_consent)
            AND consent_policy_version IS NOT NULL)
        OR (source = 'server' AND collection_basis = 'server')
    ),
    CONSTRAINT commerce_events_server_event_check CHECK (
        source = 'browser'
        OR event_name IN ('add_payment_info', 'purchase', 'refund')
    ),
    CONSTRAINT commerce_events_path_check CHECK (
        path IS NULL OR (
            path LIKE '/%' AND octet_length(path) <= 1024
            AND position('?' IN path) = 0 AND position('#' IN path) = 0
        )
    ),
    CONSTRAINT commerce_events_money_shape_check CHECK (
        (value_minor IS NULL AND currency IS NULL)
        OR (value_minor IS NOT NULL AND value_minor >= 0
            AND currency ~ '^[A-Z]{3}$')
    ),
    CONSTRAINT commerce_events_consent_check CHECK (
        NOT advertising_storage_consent OR analytics_storage_consent
    ),
    CONSTRAINT commerce_events_provider_eligibility_check CHECK (
        NOT provider_eligible
        OR (analytics_storage_consent AND advertising_storage_consent)
        OR collection_basis IN ('store_policy', 'server')
    ),
    CONSTRAINT commerce_events_policy_check CHECK (
        consent_policy_version IS NULL
        OR consent_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT commerce_events_revision_check CHECK (settings_revision > 0),
    CONSTRAINT commerce_events_properties_check CHECK (
        jsonb_typeof(properties) = 'object'
        AND octet_length(properties::text) <= 32768
    ),
    CONSTRAINT commerce_events_time_check CHECK (
        occurred_at >= received_at - INTERVAL '24 hours'
        AND occurred_at <= received_at + INTERVAL '5 minutes'
    )
) PARTITION BY RANGE (received_at);

-- pg_partman owns child partition creation. No retention parameters are set:
-- old partitions remain until an operator explicitly removes them.
SELECT partman.create_partition(
    p_parent_table := 'integration.commerce_events',
    p_control := 'received_at',
    p_interval := '1 day',
    p_premake := 7,
    p_default_table := true,
    p_automatic_maintenance := 'on',
    p_jobmon := false
);

INSERT INTO integration.commerce_events (
    id,
    event_id,
    store_id,
    sales_channel_id,
    event_name,
    source,
    collection_basis,
    schema_version,
    visitor_id,
    session_id,
    customer_id,
    product_id,
    product_variant_id,
    cart_id,
    checkout_id,
    order_id,
    payment_attempt_id,
    refund_id,
    path,
    value_minor,
    currency,
    analytics_storage_consent,
    advertising_storage_consent,
    provider_eligible,
    consent_policy_version,
    settings_revision,
    properties,
    occurred_at,
    received_at,
    created_at
)
SELECT
    id,
    event_id,
    store_id,
    sales_channel_id,
    event_name,
    source,
    collection_basis,
    schema_version,
    visitor_id,
    session_id,
    customer_id,
    product_id,
    product_variant_id,
    cart_id,
    checkout_id,
    order_id,
    payment_attempt_id,
    refund_id,
    path,
    value_minor,
    currency,
    analytics_storage_consent,
    advertising_storage_consent,
    provider_eligible,
    consent_policy_version,
    settings_revision,
    properties,
    occurred_at,
    received_at,
    created_at
FROM integration.commerce_events_legacy;

DROP TABLE integration.commerce_events_legacy;

CREATE INDEX commerce_events_visitor_path_idx
    ON integration.commerce_events (store_id, visitor_id, occurred_at, id)
    WHERE visitor_id IS NOT NULL;

CREATE INDEX commerce_events_customer_path_idx
    ON integration.commerce_events (store_id, customer_id, occurred_at, id)
    WHERE customer_id IS NOT NULL;

CREATE INDEX commerce_events_channel_time_idx
    ON integration.commerce_events (store_id, sales_channel_id, occurred_at DESC, id DESC);

CREATE INDEX commerce_events_event_key_idx
    ON integration.commerce_events (store_id, event_id);

CREATE INDEX commerce_events_store_id_idx
    ON integration.commerce_events (store_id, id DESC);

ALTER TABLE integration.commerce_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.commerce_events FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.commerce_events
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT ON integration.commerce_events TO chaos_runtime;
REVOKE UPDATE, DELETE ON integration.commerce_events FROM chaos_runtime;

-- A daily database-local job keeps the next set of partitions available.
-- It does not delete anything.
SELECT cron.schedule(
    'chaos-analytics-partition-maintenance',
    '5 0 * * *',
    'SELECT partman.run_maintenance();'
);

COMMENT ON TABLE integration.commerce_events IS
    'Append-only Store-scoped Analytics event log partitioned daily by received_at; retention is manual.';
COMMENT ON TABLE integration.analytics_event_deliveries IS
    'Provider delivery observations; remove rows before manually dropping event-log partitions.';
