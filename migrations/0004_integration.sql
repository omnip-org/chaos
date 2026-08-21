-- === Integration bootstrap ===
--
-- This is the complete integration foundation for a fresh database. The
-- bootstrap owns durable outbox/webhook delivery and the Analytics workflow;
-- notification delivery is intentionally out of scope.

-- Objects within every capability are ordered as types, tables, indexes,
-- routines, row-level security, policies, and privileges.

CREATE SCHEMA integration;

COMMENT ON SCHEMA integration IS
    'Reliable integration delivery, notifications, and analytical processing';

CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'store', 'shopper');

CREATE TYPE integration.delivery_status AS ENUM ('pending', 'processed', 'dead_letter');

SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_fulfillment_events');
SELECT pgmq.create('chaos_search_events');
SELECT pgmq.create('chaos_analytics_events');
SELECT pgmq.create('chaos_webhooks');
SELECT pgmq.create('chaos_analytics_destinations');

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

CREATE TABLE integration.webhook_inbox (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    provider             TEXT        NOT NULL,
    provider_account_id  UUID        NOT NULL,
    provider_event_id    TEXT        NOT NULL,
    event_type           TEXT        NOT NULL,
    payload              JSONB       NOT NULL,
    pgmq_message_id      BIGINT      NOT NULL UNIQUE,
    processed_at         TIMESTAMPTZ,
    failed_at            TIMESTAMPTZ,
    last_error           TEXT,
    verified_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider_account_id, provider_event_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.provider_accounts(store_id, id),
    CONSTRAINT webhook_inbox_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT webhook_inbox_completion_check CHECK (
        processed_at IS NULL OR failed_at IS NULL
    )
);

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

CREATE TABLE integration.outbox_events (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    aggregate_type       TEXT        NOT NULL,
    aggregate_id         UUID        NOT NULL,
    event_type           TEXT        NOT NULL,
    payload              JSONB       NOT NULL,
    pgmq_message_id      BIGINT      NOT NULL UNIQUE,
    processed_at         TIMESTAMPTZ,
    failed_at            TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (event_type)
        REFERENCES integration.event_consumer_registry(event_type),
    CONSTRAINT outbox_events_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT outbox_events_completion_check CHECK (
        processed_at IS NULL OR failed_at IS NULL
    )
);

CREATE INDEX webhook_inbox_claim_idx
    ON integration.webhook_inbox (created_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;

CREATE INDEX outbox_events_pending_idx
    ON integration.outbox_events (created_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;

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
           count(event.id) FILTER (
               WHERE event.processed_at IS NULL AND event.failed_at IS NULL
           ),
           0,
           count(event.id) FILTER (WHERE event.failed_at IS NOT NULL),
           count(event.id) FILTER (WHERE event.processed_at IS NOT NULL)
      FROM integration.event_consumer_registry AS registry
      LEFT JOIN integration.outbox_events AS event
        ON event.event_type = registry.event_type
     GROUP BY registry.event_type, registry.consumer_owner
     ORDER BY registry.event_type;
$$;

CREATE FUNCTION integration.event_queue_name(event_type TEXT)
RETURNS TEXT
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT CASE registry.consumer_owner
        WHEN 'payments.provider_dispatch' THEN 'chaos_payment_commands'
        WHEN 'fulfillment.operations' THEN 'chaos_fulfillment_events'
        WHEN 'search.product_indexer' THEN 'chaos_search_events'
        WHEN 'analytics.event_ingestor' THEN 'chaos_analytics_events'
    END
      FROM integration.event_consumer_registry AS registry
     WHERE registry.event_type = event_queue_name.event_type;
$$;

CREATE FUNCTION integration.enqueue_outbox_event()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    queue_name TEXT;
BEGIN
    queue_name := integration.event_queue_name(NEW.event_type);
    IF queue_name IS NULL THEN
        RAISE EXCEPTION 'event type % has no queue owner', NEW.event_type
            USING ERRCODE = '23514';
    END IF;
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          queue_name,
          jsonb_build_object('version', 1, 'event_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.enqueue_webhook_event()
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

CREATE FUNCTION integration.claim_routed_outbox_events(
    queue_name TEXT,
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    aggregate_id UUID,
    payload JSONB,
    occurred_at TIMESTAMPTZ,
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
    IF queue_name NOT IN (
        'chaos_payment_commands',
        'chaos_fulfillment_events',
        'chaos_search_events',
        'chaos_analytics_events'
    ) THEN
        RAISE EXCEPTION 'unsupported outbox queue %', queue_name
            USING ERRCODE = '22023';
    END IF;

    FOR message IN
        SELECT queued.msg_id, queued.read_ct
          FROM pgmq.read(
                   queue_name,
                   120,
                   greatest(least(batch_size, 100), 1),
                   '{}'::jsonb
               ) AS queued
    LOOP
        SELECT event.id,
               event.store_id,
               event.event_type,
               event.aggregate_id,
               event.payload,
               event.created_at
          INTO target
          FROM integration.outbox_events AS event
         WHERE event.pgmq_message_id = message.msg_id
           AND event.processed_at IS NULL
           AND event.failed_at IS NULL;
        IF NOT FOUND THEN
            PERFORM pgmq.delete(queue_name, message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        event_type := target.event_type;
        aggregate_id := target.aggregate_id;
        payload := target.payload;
        occurred_at := target.created_at;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.claim_outbox_events(
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
      FROM integration.claim_routed_outbox_events(
               'chaos_payment_commands', batch_size
           ) AS event;
$$;

CREATE FUNCTION integration.claim_fulfillment_events(
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
      FROM integration.claim_routed_outbox_events(
               'chaos_fulfillment_events', batch_size
           ) AS event;
$$;

CREATE FUNCTION integration.claim_webhook_events(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    provider TEXT,
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
               event.provider,
               event.event_type,
               event.payload
          INTO target
          FROM integration.webhook_inbox AS event
         WHERE event.pgmq_message_id = message.msg_id
           AND event.processed_at IS NULL
           AND event.failed_at IS NULL;
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_webhooks', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        provider := target.provider;
        event_type := target.event_type;
        payload := target.payload;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.finish_outbox_event(
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
    queue_name TEXT;
BEGIN
    SELECT event.pgmq_message_id,
           integration.event_queue_name(event.event_type)
      INTO message_id, queue_name
      FROM integration.outbox_events AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL OR queue_name IS NULL THEN
        RETURN false;
    END IF;

    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE integration.outbox_events AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete(queue_name, message_id);
    ELSE
        UPDATE integration.outbox_events AS event
           SET last_error = left(failure, 2000)
         WHERE event.id = event_id;
        PERFORM pgmq.set_vt(
            queue_name,
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

CREATE FUNCTION integration.finish_webhook_event(
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
      FROM integration.webhook_inbox AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE integration.webhook_inbox AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSE
        UPDATE integration.webhook_inbox AS event
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

ALTER TABLE integration.idempotency_records ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.webhook_inbox ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.outbox_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY idempotency_scope_isolation ON integration.idempotency_records
    USING (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'store' AND scope_id =
            nullif(current_setting('app.store_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    )
    WITH CHECK (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'store' AND scope_id =
            nullif(current_setting('app.store_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    );

CREATE POLICY store_isolation ON integration.webhook_inbox
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.outbox_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
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
    ('analytics.payment.initiated', 'analytics.event_ingestor',
     'Records an authoritative AddPaymentInfo event in the Commerce Event ledger'),
    ('analytics.payment.captured', 'analytics.event_ingestor',
     'Records an authoritative Purchase event in the Commerce Event ledger'),
    ('analytics.cart.line_added', 'analytics.event_ingestor',
     'Records an authoritative AddToCart event in the Commerce Event ledger'),
    ('analytics.checkout.initiated', 'analytics.event_ingestor',
     'Records an authoritative InitiateCheckout event in the Commerce Event ledger'),
    ('analytics.refund.succeeded', 'analytics.event_ingestor',
     'Records an authoritative Refund event in the Commerce Event ledger');

CREATE TRIGGER outbox_events_enqueue
BEFORE INSERT ON integration.outbox_events
FOR EACH ROW EXECUTE FUNCTION integration.enqueue_outbox_event();

CREATE TRIGGER webhook_inbox_enqueue
BEFORE INSERT ON integration.webhook_inbox
FOR EACH ROW EXECUTE FUNCTION integration.enqueue_webhook_event();

REVOKE ALL ON FUNCTION integration.event_queue_name(TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.enqueue_outbox_event() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.enqueue_webhook_event() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_routed_outbox_events(
    TEXT, INTEGER
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_outbox_events(
    INTEGER
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_fulfillment_events(
    INTEGER
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_webhook_events(
    INTEGER
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_outbox_event(
    UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_webhook_event(
    UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.event_consumer_backlog() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_outbox_events(
    INTEGER
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_fulfillment_events(
    INTEGER
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_webhook_events(
    INTEGER
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_outbox_event(
    UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_webhook_event(
    UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.event_consumer_backlog() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON integration.event_consumer_registry FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.webhook_inbox, integration.outbox_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;


CREATE INDEX webhook_inbox_provider_account_idx
    ON integration.webhook_inbox (provider_account_id, created_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;

-- === Analytics workflow ===

ALTER TABLE commerce.sales_channels
    ADD CONSTRAINT sales_channels_store_id_id_key UNIQUE (store_id, id);

CREATE TYPE integration.event_source AS ENUM ('browser', 'server');

CREATE TYPE integration.browser_collection_mode AS ENUM ('opt_in', 'opt_out');

CREATE TYPE integration.browser_collection_basis AS ENUM ('consent', 'store_policy', 'server');

CREATE TYPE integration.commerce_event_name AS ENUM (
    'page_view',
    'view_content',
    'search',
    'add_to_cart',
    'initiate_checkout',
    'add_payment_info',
    'purchase',
    'refund',
    'view_duration'
);

CREATE TABLE integration.analytics_settings (
    store_id                    UUID        NOT NULL PRIMARY KEY,
    revision                    INTEGER     NOT NULL,
    collection_enabled          BOOLEAN     NOT NULL,
    browser_collection_mode     integration.browser_collection_mode NOT NULL,
    provider_reporting_enabled  BOOLEAN     NOT NULL,
    updated_by                  UUID        NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,

    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT analytics_settings_revision_check CHECK (revision > 0)
);

ALTER TABLE integration.analytics_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_settings FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_settings
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

-- Claim the analytics outbox through the shared integration queue.
CREATE FUNCTION integration.claim_analytics_events(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    aggregate_id UUID,
    payload JSONB,
    occurred_at TIMESTAMPTZ,
    attempts INTEGER
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT event.id,
           event.store_id,
           event.event_type,
           event.aggregate_id,
           event.payload,
           event.occurred_at,
           event.attempts
      FROM integration.claim_routed_outbox_events(
               'chaos_analytics_events', batch_size
           ) AS event;
$$;

-- Partitioned append-only event ledger. It has no retention policy and no
-- foreign key into delivery rows, so operators can remove old partitions
-- deliberately after clearing delivery observations.
CREATE TABLE integration.commerce_events (
    id                          UUID                            NOT NULL,
    event_id                    UUID                            NOT NULL,
    store_id                    UUID                            NOT NULL,
    sales_channel_id            UUID                            NOT NULL,
    event_name                  integration.commerce_event_name NOT NULL,
    source                      integration.event_source         NOT NULL,
    collection_basis            integration.browser_collection_basis NOT NULL,
    schema_version              SMALLINT                        NOT NULL,
    shopper_id                  UUID,
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
        (shopper_id IS NULL OR shopper_id <> '00000000-0000-0000-0000-000000000000'::uuid)
        AND (session_id IS NULL OR session_id <> '00000000-0000-0000-0000-000000000000'::uuid)
    ),
    CONSTRAINT commerce_events_browser_shape_check CHECK (
        (source = 'browser' AND shopper_id IS NOT NULL AND session_id IS NOT NULL
            AND collection_basis IN ('consent', 'store_policy')
            AND (collection_basis = 'store_policy' OR analytics_storage_consent)
            AND consent_policy_version IS NOT NULL)
        OR (source = 'server' AND collection_basis = 'server')
    ),
    CONSTRAINT commerce_events_server_event_check CHECK (
        source = 'browser'
        OR event_name IN ('add_to_cart', 'initiate_checkout', 'add_payment_info', 'purchase', 'refund')
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

CREATE INDEX commerce_events_shopper_path_idx
    ON integration.commerce_events (store_id, shopper_id, occurred_at, id)
    WHERE shopper_id IS NOT NULL;

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

-- Generic Analytics destination configuration and delivery state.
CREATE TABLE integration.analytics_connections (
    id                          UUID        NOT NULL PRIMARY KEY,
    store_id                    UUID        NOT NULL,
    provider                    TEXT        NOT NULL,
    external_account_reference TEXT        NOT NULL,
    credential_secret_reference TEXT        NOT NULL,
    configuration               JSONB      NOT NULL DEFAULT '{}'::jsonb,
    enabled                     BOOLEAN     NOT NULL,
    created_by                  UUID        NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,

    UNIQUE (store_id, id),
    UNIQUE (store_id, provider),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT analytics_connections_provider_check CHECK (
        provider ~ '^[a-z][a-z0-9_]{1,31}$'
    ),
    CONSTRAINT analytics_connections_account_check CHECK (
        octet_length(external_account_reference) BETWEEN 1 AND 255
    ),
    CONSTRAINT analytics_connections_secret_check CHECK (
        credential_secret_reference ~
            '^(enc://[A-Za-z0-9_-]+|env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96})$'
        AND octet_length(credential_secret_reference) <= 518
    ),
    CONSTRAINT analytics_connections_configuration_check CHECK (
        jsonb_typeof(configuration) = 'object'
        AND octet_length(configuration::text) <= 16384
    )
);

CREATE TABLE integration.analytics_event_deliveries (
    id                  UUID        NOT NULL PRIMARY KEY,
    store_id            UUID        NOT NULL,
    connection_id       UUID        NOT NULL,
    commerce_event_id   UUID        NOT NULL,
    delivery_status     integration.delivery_status NOT NULL DEFAULT 'pending',
    pgmq_message_id     BIGINT      NOT NULL UNIQUE,
    delivered_at        TIMESTAMPTZ,
    provider_reference  TEXT,
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,

    UNIQUE (store_id, id),
    UNIQUE (store_id, connection_id, commerce_event_id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, connection_id)
        REFERENCES integration.analytics_connections(store_id, id) ON DELETE CASCADE,
    CONSTRAINT analytics_event_deliveries_completion_check CHECK (
        (delivery_status = 'processed' AND delivered_at IS NOT NULL)
        OR (delivery_status <> 'processed' AND delivered_at IS NULL)
    ),
    CONSTRAINT analytics_event_deliveries_reference_check CHECK (
        provider_reference IS NULL OR octet_length(provider_reference) <= 512
    ),
    CONSTRAINT analytics_event_deliveries_error_check CHECK (
        last_error IS NULL OR octet_length(last_error) <= 2048
    )
);

CREATE INDEX analytics_event_deliveries_claim_idx
    ON integration.analytics_event_deliveries (created_at, id)
    WHERE delivery_status = 'pending';

CREATE FUNCTION integration.claim_analytics_event_deliveries(
    batch_size INTEGER
)
RETURNS TABLE (id UUID, store_id UUID, connection_id UUID, commerce_event_id UUID, attempts INTEGER)
LANGUAGE plpgsql
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
                   'chaos_analytics_destinations',
                   120,
                   greatest(least(batch_size, 100), 1),
                   '{}'::jsonb
               ) AS queued
    LOOP
        SELECT delivery.id, delivery.store_id, delivery.connection_id,
               delivery.commerce_event_id
          INTO target
          FROM integration.analytics_event_deliveries AS delivery
         WHERE delivery.pgmq_message_id = message.msg_id
           AND delivery.delivery_status = 'pending';
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_analytics_destinations', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        connection_id := target.connection_id;
        commerce_event_id := target.commerce_event_id;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.enqueue_analytics_event_delivery()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          'chaos_analytics_destinations',
          jsonb_build_object('version', 1, 'delivery_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.finish_analytics_event_delivery(
    delivery_id UUID,
    attempts INTEGER,
    succeeded BOOLEAN,
    retryable BOOLEAN,
    provider_reference TEXT,
    failure TEXT,
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
    SELECT delivery.pgmq_message_id
      INTO message_id
      FROM integration.analytics_event_deliveries AS delivery
     WHERE delivery.id = delivery_id
       AND delivery.delivery_status = 'pending'
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR NOT retryable OR attempts >= 8 THEN
        UPDATE integration.analytics_event_deliveries AS delivery
           SET delivery_status = CASE
                   WHEN succeeded THEN 'processed'::integration.delivery_status
                   ELSE 'dead_letter'::integration.delivery_status
               END,
               delivered_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               provider_reference = finish_analytics_event_delivery.provider_reference,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2048) END,
               updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.delete('chaos_analytics_destinations', message_id);
    ELSE
        UPDATE integration.analytics_event_deliveries AS delivery
           SET last_error = left(failure, 2048), updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.set_vt(
            'chaos_analytics_destinations',
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

CREATE TRIGGER analytics_event_deliveries_enqueue
BEFORE INSERT ON integration.analytics_event_deliveries
FOR EACH ROW EXECUTE FUNCTION integration.enqueue_analytics_event_delivery();

ALTER TABLE integration.analytics_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_connections FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_deliveries FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_connections
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_event_deliveries
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.claim_analytics_event_deliveries(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_analytics_event_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_analytics_event_delivery() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_analytics_event_deliveries(INTEGER)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_analytics_event_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.analytics_connections,
       integration.analytics_event_deliveries
    FROM chaos_runtime;

COMMENT ON TABLE integration.analytics_connections IS
    'Store-scoped configuration for an external analytics destination.';
COMMENT ON TABLE integration.analytics_event_deliveries IS
    'Durable delivery state for an Analytics event and external destination.';

-- Cross-Store scheduling is kept behind one reviewed SECURITY DEFINER routine.
CREATE FUNCTION integration.schedule_analytics_event_deliveries(
    batch_size INTEGER
)
RETURNS BIGINT
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    scheduled BIGINT;
BEGIN
    WITH candidates AS (
        SELECT event.store_id,
               event.id AS commerce_event_id,
               connection.id AS connection_id
          FROM integration.commerce_events AS event
          JOIN integration.analytics_connections AS connection
            ON connection.store_id = event.store_id
           AND connection.enabled
         WHERE event.provider_eligible
           AND NOT EXISTS (
               SELECT 1
                 FROM integration.analytics_event_deliveries AS delivery
                WHERE delivery.store_id = event.store_id
                  AND delivery.connection_id = connection.id
                  AND delivery.commerce_event_id = event.id
           )
         ORDER BY event.received_at, event.id, connection.id
         LIMIT greatest(least(batch_size, 100), 0)
    )
    INSERT INTO integration.analytics_event_deliveries (
        id, store_id, connection_id, commerce_event_id, created_at, updated_at
    )
    SELECT uuidv7(), store_id, connection_id, commerce_event_id,
           CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
      FROM candidates
    ON CONFLICT (store_id, connection_id, commerce_event_id) DO NOTHING;

    GET DIAGNOSTICS scheduled = ROW_COUNT;
    RETURN scheduled;
END;
$$;

REVOKE ALL ON FUNCTION integration.schedule_analytics_event_deliveries(INTEGER)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION integration.schedule_analytics_event_deliveries(INTEGER)
    TO chaos_runtime;

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

-- === Cross-schema reliability constraints ===

ALTER TABLE commerce.order_fulfillment_transitions
    ADD CONSTRAINT order_fulfillment_transitions_source_event_id_fkey
    FOREIGN KEY (source_event_id) REFERENCES integration.outbox_events(id);

-- === Runtime hardening ===

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

REVOKE UPDATE, DELETE
    ON commerce.stock_ledger_entries,
       commerce.customer_shopper_links,
       commerce.checkout_contacts,
       commerce.checkout_addresses,
       commerce.checkout_lines,
       commerce.checkout_tax_calculations,
       commerce.checkout_promotion_calculations,
       commerce.checkout_shipping_selections,
       commerce.order_contacts,
       commerce.order_addresses,
       commerce.order_lines,
       commerce.order_tax_calculations,
       commerce.order_promotion_calculations,
       commerce.order_shipping_selections,
       commerce.order_transitions,
       commerce.order_fulfillment_transitions
    FROM chaos_runtime;

REVOKE DELETE
    ON commerce.collections,
       commerce.media_assets,
       commerce.reviews,
       commerce.checkouts,
       commerce.orders
    FROM chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON integration.event_consumer_registry FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.webhook_inbox,
       integration.outbox_events,
       integration.commerce_events,
       integration.analytics_event_deliveries
    FROM chaos_runtime;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';

COMMENT ON ROLE chaos_identity IS
    'Non-owner identity role. It cannot access Store-owned commerce tables.';


REVOKE ALL ON FUNCTION integration.claim_analytics_events(INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION integration.claim_analytics_events(INTEGER) TO chaos_runtime;

COMMENT ON TABLE integration.analytics_settings IS
    'Store-level collection and provider reporting policy.';
