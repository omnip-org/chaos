-- === Reliable delivery foundation ===

CREATE SCHEMA integration;

COMMENT ON SCHEMA integration IS
    'Reliable integration delivery, notifications, and analytical processing';

CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'store', 'shopper');

CREATE TYPE integration.queue_status AS ENUM ('pending', 'processing', 'processed', 'dead_letter');

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
    id                         UUID                     NOT NULL PRIMARY KEY,
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
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
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
    id                   UUID                     NOT NULL PRIMARY KEY,
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

    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
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

CREATE INDEX webhook_inbox_claim_idx
    ON integration.webhook_inbox (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE INDEX outbox_events_claim_idx
    ON integration.outbox_events (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

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

CREATE FUNCTION integration.claim_outbox_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
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
    RETURNING event.id, event.store_id,
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
    RETURNING event.id, event.store_id,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.claim_webhook_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
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
    RETURNING event.id, event.store_id, event.provider,
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

REVOKE ALL ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_fulfillment_events(
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

REVOKE ALL ON FUNCTION integration.event_consumer_backlog() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_fulfillment_events(
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

GRANT EXECUTE ON FUNCTION integration.queue_metrics() TO chaos_runtime;

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

-- === Notifications ===

CREATE TYPE integration.email_delivery_status AS ENUM (
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

CREATE TYPE integration.email_suppression_reason AS ENUM (
    'hard_bounce',
    'complaint',
    'provider_suppression',
    'manual'
);

CREATE TABLE integration.email_deliveries (
    id                       UUID                               NOT NULL PRIMARY KEY,
    store_id                 UUID                               NOT NULL,
    semantic_event_id        UUID                               NOT NULL,
    semantic_event_type      TEXT                               NOT NULL,
    recipient_email          extensions.citext                  NOT NULL,
    template_key             TEXT                               NOT NULL,
    template_version         INTEGER                            NOT NULL,
    template_payload         JSONB                              NOT NULL,
    provider                 TEXT                               NOT NULL DEFAULT 'resend',
    provider_message_id      TEXT,
    delivery_status          integration.email_delivery_status NOT NULL DEFAULT 'pending',
    attempts                 INTEGER                            NOT NULL DEFAULT 0,
    available_at             TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by                UUID,
    locked_at                TIMESTAMPTZ,
    sent_at                  TIMESTAMPTZ,
    delivered_at             TIMESTAMPTZ,
    last_error               TEXT,
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, semantic_event_id),
    UNIQUE (provider, provider_message_id),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
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

CREATE TABLE integration.email_suppressions (
    id                    UUID                                      NOT NULL PRIMARY KEY,
    store_id              UUID                                      NOT NULL,
    recipient_email       extensions.citext                         NOT NULL,
    suppression_reason    integration.email_suppression_reason     NOT NULL,
    source_delivery_id    UUID,
    created_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, recipient_email),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, source_delivery_id)
        REFERENCES integration.email_deliveries(store_id, id),
    CONSTRAINT email_suppressions_recipient_length_check CHECK (
        length(recipient_email::text) BETWEEN 3 AND 320
    )
);

CREATE TABLE integration.webhook_events (
    id                    UUID                     NOT NULL PRIMARY KEY,
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
    FOREIGN KEY (store_id, delivery_id)
        REFERENCES integration.email_deliveries(store_id, id),
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

CREATE INDEX email_deliveries_claim_idx
    ON integration.email_deliveries (delivery_status, available_at, created_at, id)
    WHERE delivery_status IN ('pending', 'processing');

CREATE INDEX email_deliveries_recipient_idx
    ON integration.email_deliveries (store_id,
        recipient_email,
        created_at DESC,
        id DESC
    );

CREATE INDEX notification_webhook_events_delivery_idx
    ON integration.webhook_events (store_id,
        delivery_id,
        received_at,
        id
    );

CREATE FUNCTION integration.email_delivery_metrics()
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
      FROM integration.email_deliveries AS delivery;
$$;

CREATE FUNCTION integration.claim_email_deliveries(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
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
        UPDATE integration.email_deliveries AS delivery
           SET delivery_status = 'suppressed',
               locked_by = NULL,
               locked_at = NULL,
               last_error = 'recipient is suppressed',
               updated_at = claimed_at
         WHERE delivery.delivery_status IN ('pending', 'processing')
           AND EXISTS (
               SELECT 1
                 FROM integration.email_suppressions AS suppression
                WHERE suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
        RETURNING delivery.id
    ), expired AS (
        UPDATE integration.email_deliveries AS delivery
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
          FROM integration.email_deliveries AS delivery
         WHERE (
                 (delivery.delivery_status = 'pending' AND delivery.available_at <= claimed_at)
                 OR (delivery.delivery_status = 'processing' AND delivery.locked_at <= stale_before)
               )
           AND delivery.attempts < 8
           AND NOT EXISTS (
               SELECT 1
                 FROM integration.email_suppressions AS suppression
                WHERE suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
         ORDER BY delivery.available_at, delivery.created_at, delivery.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.email_deliveries AS delivery
       SET delivery_status = 'processing',
           attempts = delivery.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at,
           updated_at = claimed_at
      FROM claimable
     WHERE delivery.id = claimable.id
    RETURNING delivery.id, delivery.store_id,
              delivery.recipient_email::text, delivery.template_key,
              delivery.template_version, delivery.template_payload,
              delivery.provider, delivery.attempts;
$$;

CREATE FUNCTION integration.finish_email_delivery(
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
    UPDATE integration.email_deliveries AS delivery
       SET delivery_status = CASE
               WHEN succeeded THEN 'sent'::integration.email_delivery_status
               WHEN NOT retryable THEN 'failed'::integration.email_delivery_status
               WHEN delivery.attempts >= 8 THEN 'dead_letter'::integration.email_delivery_status
               ELSE 'pending'::integration.email_delivery_status
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

CREATE FUNCTION integration.record_resend_webhook(
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
    target integration.email_deliveries%ROWTYPE;
    webhook_id UUID;
    suppression_reason integration.email_suppression_reason;
BEGIN
    SELECT delivery.*
      INTO target
      FROM integration.email_deliveries AS delivery
     WHERE delivery.provider = 'resend'
       AND delivery.provider_message_id = record_resend_webhook.provider_message_id
     FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    webhook_id := uuidv7();
    INSERT INTO integration.webhook_events (
        id, store_id, delivery_id, provider, provider_event_id,
        provider_event_type, payload, received_at, processed_at
    ) VALUES (
        webhook_id, target.store_id, target.id, 'resend',
        record_resend_webhook.provider_event_id, record_resend_webhook.provider_event_type,
        payload, received_at, received_at
    ) ON CONFLICT ON CONSTRAINT webhook_events_provider_provider_event_id_key DO NOTHING;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    IF provider_event_type = 'email.sent'
       AND target.delivery_status IN ('pending', 'processing', 'sent') THEN
        UPDATE integration.email_deliveries
           SET delivery_status = 'sent', updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.delivered'
          AND target.delivery_status NOT IN ('bounced', 'complained', 'suppressed') THEN
        UPDATE integration.email_deliveries
           SET delivery_status = 'delivered', delivered_at = received_at, updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.bounced' THEN
        UPDATE integration.email_deliveries
           SET delivery_status = 'bounced', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status NOT IN ('complained', 'suppressed');
        IF lower(COALESCE(payload #>> '{data,bounce,type}', '')) = 'permanent' THEN
            suppression_reason := 'hard_bounce';
        END IF;
    ELSIF provider_event_type = 'email.complained' THEN
        UPDATE integration.email_deliveries
           SET delivery_status = 'complained', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status <> 'suppressed';
        suppression_reason := 'complaint';
    ELSIF provider_event_type = 'email.suppressed' THEN
        UPDATE integration.email_deliveries
           SET delivery_status = 'suppressed', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id;
        suppression_reason := 'provider_suppression';
    END IF;

    IF suppression_reason IS NOT NULL THEN
        INSERT INTO integration.email_suppressions (
            id, store_id, recipient_email, suppression_reason,
            source_delivery_id, created_at, updated_at
        ) VALUES (
            uuidv7(), target.store_id, target.recipient_email,
            suppression_reason, target.id, received_at, received_at
        ) ON CONFLICT (store_id, recipient_email) DO UPDATE
            SET suppression_reason = CASE
                    WHEN integration.email_suppressions.suppression_reason = 'manual'
                        THEN integration.email_suppressions.suppression_reason
                    WHEN integration.email_suppressions.suppression_reason = 'complaint'
                        THEN integration.email_suppressions.suppression_reason
                    WHEN EXCLUDED.suppression_reason = 'complaint'
                        THEN EXCLUDED.suppression_reason
                    WHEN integration.email_suppressions.suppression_reason = 'hard_bounce'
                        THEN integration.email_suppressions.suppression_reason
                    ELSE EXCLUDED.suppression_reason
                END,
                source_delivery_id = CASE
                    WHEN integration.email_suppressions.suppression_reason IN ('manual', 'complaint')
                        THEN integration.email_suppressions.source_delivery_id
                    ELSE EXCLUDED.source_delivery_id
                END,
                updated_at = EXCLUDED.updated_at;
    END IF;
    RETURN true;
END;
$$;

ALTER TABLE integration.email_deliveries ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.email_suppressions ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.webhook_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.email_deliveries
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.email_suppressions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.webhook_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION integration.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.email_delivery_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.email_delivery_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.email_deliveries, integration.email_suppressions,
       integration.webhook_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;

-- === Analytics ===

CREATE TYPE integration.event_source AS ENUM ('browser', 'server');

CREATE TYPE integration.browser_event_name AS ENUM (
    'page_viewed',
    'product_viewed',
    'search_performed',
    'cart_line_added',
    'checkout_started',
    'engagement_heartbeat'
);

CREATE TYPE integration.erasure_status AS ENUM ('pending', 'completed');

CREATE TYPE integration.commerce_fact_name AS ENUM (
    'order_created',
    'payment_captured',
    'refund_succeeded',
    'fulfillment_shipped',
    'return_completed'
);

CREATE TYPE integration.attribution_model AS ENUM ('first_touch', 'last_touch');

CREATE TYPE integration.destination_provider AS ENUM ('meta_capi', 'ga4');

CREATE TABLE integration.store_policy_versions (
    id                              UUID        NOT NULL PRIMARY KEY,
    store_id                        UUID        NOT NULL,
    version                         INTEGER     NOT NULL,
    behavior_collection_enabled     BOOLEAN     NOT NULL,
    advertising_exports_enabled     BOOLEAN     NOT NULL,
    identity_linking_enabled        BOOLEAN     NOT NULL,
    raw_event_retention_days        SMALLINT    NOT NULL,
    created_by                      UUID        NOT NULL,
    effective_at                    TIMESTAMPTZ NOT NULL,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, version),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (created_by) REFERENCES identity.users(id),
    CONSTRAINT store_policy_versions_version_check CHECK (version BETWEEN 1 AND 2147483647),
    CONSTRAINT store_policy_versions_retention_check CHECK (
        raw_event_retention_days BETWEEN 1 AND 400
    )
);

CREATE TABLE integration.identity_links (
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    anonymous_id               UUID        NOT NULL,
    customer_id                UUID        NOT NULL,
    consent_policy_version     TEXT        NOT NULL,
    collection_policy_version  TEXT        NOT NULL,
    linked_at                  TIMESTAMPTZ NOT NULL,
    retention_expires_at       TIMESTAMPTZ NOT NULL,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, anonymous_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, customer_id)
        REFERENCES commerce.customers(store_id, id) ON DELETE CASCADE,
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

CREATE TABLE integration.behavior_events (
    id                            UUID                         NOT NULL PRIMARY KEY,
    event_id                      UUID                         NOT NULL,
    store_id                      UUID                         NOT NULL,
    sales_channel_id              UUID                         NOT NULL,
    event_name                    integration.browser_event_name NOT NULL,
    schema_version                SMALLINT                     NOT NULL,
    source                        integration.event_source       NOT NULL,
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

    UNIQUE (store_id, id),
    UNIQUE (store_id, event_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
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

CREATE TABLE integration.behavior_event_processing (
    id                    UUID                     NOT NULL PRIMARY KEY,
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

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, id)
        REFERENCES integration.behavior_events(store_id, id) ON DELETE CASCADE,
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

CREATE TABLE integration.sessions (
    id                               UUID        NOT NULL PRIMARY KEY,
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

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
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

CREATE TABLE integration.erasure_requests (
    id                       UUID                      NOT NULL PRIMARY KEY,
    store_id                 UUID                      NOT NULL,
    anonymous_id             UUID,
    customer_id              UUID,
    status                   integration.erasure_status NOT NULL DEFAULT 'pending',
    requested_by             UUID                      NOT NULL,
    behavior_events_deleted  BIGINT                    NOT NULL DEFAULT 0,
    attribution_results_deleted BIGINT                 NOT NULL DEFAULT 0,
    sessions_deleted         BIGINT                    NOT NULL DEFAULT 0,
    identity_links_deleted   BIGINT                    NOT NULL DEFAULT 0,
    requested_at             TIMESTAMPTZ               NOT NULL,
    completed_at             TIMESTAMPTZ,
    created_at               TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
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

CREATE TABLE integration.commerce_facts (
    id                    UUID                          NOT NULL PRIMARY KEY,
    store_id              UUID                          NOT NULL,
    sales_channel_id      UUID                          NOT NULL,
    fact_name             integration.commerce_fact_name NOT NULL,
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

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
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

CREATE TABLE integration.attribution_jobs (
    commerce_fact_id     UUID                     NOT NULL,
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

    PRIMARY KEY (store_id, commerce_fact_id, model_version),
    FOREIGN KEY (store_id, commerce_fact_id)
        REFERENCES integration.commerce_facts(store_id, id) ON DELETE CASCADE,
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

CREATE TABLE integration.attribution_results (
    id                            UUID                        NOT NULL PRIMARY KEY,
    store_id                      UUID                        NOT NULL,
    sales_channel_id              UUID                        NOT NULL,
    commerce_fact_id              UUID                        NOT NULL,
    order_id                      UUID                        NOT NULL,
    customer_id                   UUID,
    checkout_id                   UUID                        NOT NULL,
    cart_id                       UUID                        NOT NULL,
    attribution_model             integration.attribution_model NOT NULL,
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

    UNIQUE (store_id, commerce_fact_id, attribution_model, model_version),
    FOREIGN KEY (store_id, commerce_fact_id)
        REFERENCES integration.commerce_facts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, touch_event_id)
        REFERENCES integration.behavior_events(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id),
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

CREATE TABLE integration.daily_behavior_reports (
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

    PRIMARY KEY (store_id, sales_channel_id, report_date),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE,
    CONSTRAINT daily_behavior_reports_counts_check CHECK (
        sessions >= 0 AND events >= 0 AND page_views >= 0 AND product_views >= 0
        AND searches >= 0 AND cart_line_additions >= 0 AND checkouts_started >= 0
        AND active_engagement_milliseconds >= 0
    )
);

CREATE TABLE integration.daily_commerce_reports (
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

    PRIMARY KEY (store_id, sales_channel_id, report_date, currency),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE,
    CONSTRAINT daily_commerce_reports_currency_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT daily_commerce_reports_counts_check CHECK (
        orders_created >= 0 AND order_amount_minor >= 0
        AND payments_captured >= 0 AND captured_amount_minor >= 0
        AND refunds_succeeded >= 0 AND refunded_amount_minor >= 0
        AND fulfillments_shipped >= 0 AND returns_completed >= 0
    )
);

CREATE TABLE integration.daily_attribution_reports (
    store_id             UUID                         NOT NULL,
    sales_channel_id     UUID                         NOT NULL,
    report_date          DATE                         NOT NULL,
    attribution_model    integration.attribution_model  NOT NULL,
    model_version        SMALLINT                     NOT NULL,
    is_direct            BOOLEAN                      NOT NULL,
    campaign_source      TEXT                         NOT NULL DEFAULT '',
    campaign_medium      TEXT                         NOT NULL DEFAULT '',
    campaign_name        TEXT                         NOT NULL DEFAULT '',
    attributed_orders    BIGINT                       NOT NULL,
    attributed_amount_minor BIGINT                    NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    refreshed_at         TIMESTAMPTZ                  NOT NULL,

    PRIMARY KEY (store_id, sales_channel_id, report_date,
        attribution_model, model_version, is_direct,
        campaign_source, campaign_medium, campaign_name, currency
    ),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.sales_channels(id) ON DELETE CASCADE,
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

CREATE TABLE integration.destination_accounts (
    id                              UUID                           NOT NULL PRIMARY KEY,
    store_id                        UUID                           NOT NULL,
    provider                        integration.destination_provider NOT NULL,
    external_destination_reference  TEXT                           NOT NULL,
    event_source_base_url           TEXT,
    credential_secret_reference     TEXT                           NOT NULL,
    enabled                         BOOLEAN                        NOT NULL,
    created_by                      UUID                           NOT NULL,
    created_at                      TIMESTAMPTZ                    NOT NULL,
    updated_at                      TIMESTAMPTZ                    NOT NULL,

    UNIQUE (store_id, id),
    UNIQUE (store_id, provider),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (created_by) REFERENCES identity.users(id),
    CONSTRAINT destination_accounts_reference_check CHECK (
        (provider = 'meta_capi'
            AND external_destination_reference ~ '^[0-9]{5,32}$'
            AND event_source_base_url ~ '^https://[^?#]+/$'
            AND octet_length(event_source_base_url) <= 2048)
        OR (provider = 'ga4'
            AND external_destination_reference ~ '^G-[A-Z0-9]{5,20}$'
            AND event_source_base_url IS NULL)
    ),
    CONSTRAINT destination_accounts_secret_reference_check CHECK (
        credential_secret_reference ~ '^env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    )
);

CREATE TABLE integration.export_deliveries (
    id                    UUID                     NOT NULL PRIMARY KEY,
    store_id              UUID                     NOT NULL,
    destination_id        UUID                     NOT NULL,
    commerce_fact_id      UUID                     NOT NULL,
    delivery_status       integration.queue_status NOT NULL DEFAULT 'pending',
    attempts              INTEGER                  NOT NULL DEFAULT 0,
    available_at          TIMESTAMPTZ              NOT NULL,
    locked_by             UUID,
    locked_at             TIMESTAMPTZ,
    delivered_at          TIMESTAMPTZ,
    provider_reference    TEXT,
    last_error            TEXT,
    created_at            TIMESTAMPTZ              NOT NULL,
    updated_at            TIMESTAMPTZ              NOT NULL,

    UNIQUE (store_id, id),
    UNIQUE (store_id, destination_id, commerce_fact_id),
    FOREIGN KEY (store_id, destination_id)
        REFERENCES integration.destination_accounts(store_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, commerce_fact_id)
        REFERENCES integration.commerce_facts(store_id, id)
        ON DELETE CASCADE,
    CONSTRAINT export_deliveries_attempts_check CHECK (attempts BETWEEN 0 AND 31),
    CONSTRAINT export_deliveries_lease_shape_check CHECK (
        (delivery_status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (delivery_status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    ),
    CONSTRAINT export_deliveries_completion_shape_check CHECK (
        (delivery_status = 'processed' AND delivered_at IS NOT NULL
            AND provider_reference IS NOT NULL)
        OR (delivery_status <> 'processed' AND delivered_at IS NULL
            AND provider_reference IS NULL)
    ),
    CONSTRAINT export_deliveries_provider_reference_check CHECK (
        provider_reference IS NULL OR length(provider_reference) BETWEEN 1 AND 255
    ),
    CONSTRAINT export_deliveries_error_check CHECK (
        last_error IS NULL OR length(last_error) BETWEEN 1 AND 2000
    )
);

CREATE INDEX store_policy_versions_current_idx
    ON integration.store_policy_versions (store_id,
        effective_at DESC,
        version DESC
    );

CREATE INDEX identity_links_customer_idx
    ON integration.identity_links (store_id,
        customer_id,
        linked_at,
        id
    );

CREATE INDEX identity_links_retention_idx
    ON integration.identity_links (retention_expires_at, id);

CREATE INDEX behavior_events_session_time_idx
    ON integration.behavior_events (store_id,
        sales_channel_id,
        session_id,
        occurred_at,
        event_id
    );

CREATE INDEX behavior_events_retention_idx
    ON integration.behavior_events (retention_expires_at,
        id
    );

CREATE INDEX behavior_events_attribution_touch_idx
    ON integration.behavior_events (store_id,
        sales_channel_id,
        anonymous_id,
        session_id,
        occurred_at,
        id
    ) WHERE event_name = 'page_viewed';

CREATE INDEX behavior_events_checkout_attribution_idx
    ON integration.behavior_events (store_id,
        sales_channel_id,
        checkout_id,
        cart_id,
        occurred_at DESC,
        id DESC
    ) WHERE event_name = 'checkout_started';

CREATE INDEX behavior_event_processing_claim_idx
    ON integration.behavior_event_processing (processing_status, available_at, created_at, id)
    WHERE processing_status IN ('pending', 'processing');

CREATE INDEX sessions_identity_time_idx
    ON integration.sessions (store_id,
        sales_channel_id,
        anonymous_id,
        client_session_id,
        last_event_at DESC,
        id
    );

CREATE INDEX sessions_retention_idx
    ON integration.sessions (retention_expires_at, id);

CREATE INDEX erasure_requests_pending_idx
    ON integration.erasure_requests (status, requested_at, id)
    WHERE status = 'pending';

CREATE INDEX commerce_facts_store_time_idx
    ON integration.commerce_facts (store_id, occurred_at DESC, id DESC
    );

CREATE INDEX commerce_facts_store_name_time_idx
    ON integration.commerce_facts (store_id, fact_name, occurred_at DESC, id DESC
    );

CREATE INDEX attribution_jobs_claim_idx
    ON integration.attribution_jobs (processing_status, available_at, created_at, commerce_fact_id)
    WHERE processing_status IN ('pending', 'processing');

CREATE INDEX attribution_results_order_idx
    ON integration.attribution_results (store_id, order_id, model_version, attribution_model
    );

CREATE INDEX attribution_results_destination_idx
    ON integration.attribution_results (store_id, attributed_at, id
    ) WHERE advertising_export_eligible;

CREATE INDEX daily_behavior_reports_store_date_idx
    ON integration.daily_behavior_reports (store_id, report_date DESC, sales_channel_id
    );

CREATE INDEX daily_commerce_reports_store_date_idx
    ON integration.daily_commerce_reports (store_id, report_date DESC, sales_channel_id, currency
    );

CREATE INDEX daily_attribution_reports_store_date_idx
    ON integration.daily_attribution_reports (store_id, report_date DESC,
        attribution_model, model_version, sales_channel_id
    );

CREATE INDEX export_deliveries_claim_idx
    ON integration.export_deliveries (delivery_status, available_at, created_at, id)
    WHERE delivery_status IN ('pending', 'processing');

CREATE FUNCTION integration.sessionization_metrics()
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
      FROM integration.behavior_event_processing AS job;
$$;

CREATE FUNCTION integration.attribution_metrics()
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
      FROM integration.attribution_jobs AS job;
$$;

CREATE FUNCTION integration.export_delivery_metrics()
RETURNS TABLE (
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT count(*) FILTER (WHERE delivery.delivery_status = 'pending'),
           count(*) FILTER (WHERE delivery.delivery_status = 'processing'),
           count(*) FILTER (WHERE delivery.delivery_status = 'dead_letter'),
           COALESCE(extract(epoch FROM CURRENT_TIMESTAMP -
               min(delivery.available_at) FILTER (
                   WHERE delivery.delivery_status = 'pending'
               )), 0)::DOUBLE PRECISION
      FROM integration.export_deliveries AS delivery;
$$;

CREATE FUNCTION integration.retention_metrics()
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
    SELECT (SELECT count(*) FROM integration.behavior_events AS event
             WHERE event.retention_expires_at <= CURRENT_TIMESTAMP),
           (SELECT count(*) FROM integration.sessions AS session
             WHERE session.retention_expires_at <= CURRENT_TIMESTAMP),
           (SELECT count(*) FROM integration.identity_links AS link
             WHERE link.retention_expires_at <= CURRENT_TIMESTAMP),
           greatest(
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(event.retention_expires_at))
                     FROM integration.behavior_events AS event
                    WHERE event.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0),
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(session.retention_expires_at))
                     FROM integration.sessions AS session
                    WHERE session.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0),
               COALESCE((
                   SELECT extract(epoch FROM CURRENT_TIMESTAMP - min(link.retention_expires_at))
                     FROM integration.identity_links AS link
                    WHERE link.retention_expires_at <= CURRENT_TIMESTAMP
               ), 0)
           )::DOUBLE PRECISION;
$$;

CREATE FUNCTION integration.apply_store_retention_policy(
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
        UPDATE integration.behavior_events AS event
           SET retention_expires_at = least(
                   event.retention_expires_at,
                   event.received_at + make_interval(days => retention_days)
               )
         WHERE event.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_store_id =
               nullif(current_setting('app.store_id', true), '')::uuid
        RETURNING 1
    ), updated_sessions AS (
        UPDATE integration.sessions AS session
           SET retention_expires_at = least(
                   session.retention_expires_at,
                   session.last_event_at + make_interval(days => retention_days)
               ),
               updated_at = CURRENT_TIMESTAMP
         WHERE session.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_store_id =
               nullif(current_setting('app.store_id', true), '')::uuid
        RETURNING 1
    ), updated_links AS (
        UPDATE integration.identity_links AS link
           SET retention_expires_at = least(
                   link.retention_expires_at,
                   link.linked_at + make_interval(days => retention_days)
               )
         WHERE link.store_id = requested_store_id
           AND retention_days BETWEEN 1 AND 400
           AND requested_store_id =
               nullif(current_setting('app.store_id', true), '')::uuid
        RETURNING 1
    )
    SELECT (SELECT count(*) FROM updated_events),
           (SELECT count(*) FROM updated_sessions),
           (SELECT count(*) FROM updated_links);
$$;

CREATE FUNCTION integration.purge_expired_data(batch_size INTEGER, purged_at TIMESTAMPTZ)
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
          FROM integration.sessions AS session
         WHERE session.retention_expires_at <= purged_at
         ORDER BY session.retention_expires_at, session.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), deleted_sessions AS (
        DELETE FROM integration.sessions AS session
         USING expired_sessions
         WHERE session.id = expired_sessions.id
        RETURNING 1
    ), expired_events AS (
        SELECT event.id
          FROM integration.behavior_events AS event
         WHERE event.retention_expires_at <= purged_at
         ORDER BY event.retention_expires_at, event.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), expired_attributions AS (
        SELECT count(*) AS deleted_count
          FROM integration.attribution_results AS result
          INNER JOIN expired_events ON expired_events.id = result.touch_event_id
    ), deleted_events AS (
        DELETE FROM integration.behavior_events AS event
         USING expired_events
         WHERE event.id = expired_events.id
        RETURNING 1
    ), expired_links AS (
        SELECT link.id
          FROM integration.identity_links AS link
         WHERE link.retention_expires_at <= purged_at
         ORDER BY link.retention_expires_at, link.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 1000), 1)
    ), deleted_links AS (
        DELETE FROM integration.identity_links AS link
         USING expired_links
         WHERE link.id = expired_links.id
        RETURNING 1
    )
    SELECT (SELECT count(*) FROM deleted_events),
           (SELECT deleted_count FROM expired_attributions),
           (SELECT count(*) FROM deleted_sessions),
           (SELECT count(*) FROM deleted_links);
$$;

CREATE FUNCTION integration.process_erasure_requests(batch_size INTEGER, processed_at TIMESTAMPTZ)
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
               request.store_id,
               request.anonymous_id,
               request.customer_id
          FROM integration.erasure_requests AS request
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
                FROM integration.identity_links AS link
               WHERE link.store_id = request_row.store_id
                 AND link.customer_id = request_row.customer_id
          ) AS target;

        SELECT count(*)
          INTO request_attribution_results
          FROM integration.attribution_results AS result
         WHERE result.store_id = request_row.store_id
           AND result.anonymous_id = ANY(target_anonymous_ids);

        DELETE FROM integration.behavior_events AS event
         WHERE event.store_id = request_row.store_id
           AND event.anonymous_id = ANY(target_anonymous_ids);
        GET DIAGNOSTICS request_behavior_events = ROW_COUNT;

        DELETE FROM integration.sessions AS session
         WHERE session.store_id = request_row.store_id
           AND session.anonymous_id = ANY(target_anonymous_ids);
        GET DIAGNOSTICS request_sessions = ROW_COUNT;

        DELETE FROM integration.identity_links AS link
         WHERE link.store_id = request_row.store_id
           AND (
               link.anonymous_id = ANY(target_anonymous_ids)
               OR (
                   request_row.customer_id IS NOT NULL
                   AND link.customer_id = request_row.customer_id
               )
           );
        GET DIAGNOSTICS request_identity_links = ROW_COUNT;

        UPDATE integration.erasure_requests AS request
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

CREATE FUNCTION integration.erasure_metrics()
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
      FROM integration.erasure_requests AS request;
$$;

CREATE FUNCTION integration.claim_commerce_fact_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
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
    RETURNING event.id, event.store_id,
              event.event_type, event.payload, event.attempts, event.created_at;
$$;

CREATE FUNCTION integration.claim_attribution_jobs(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    commerce_fact_id UUID,
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
        UPDATE integration.attribution_jobs AS job
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
        SELECT job.store_id,
               job.commerce_fact_id,
               job.model_version
          FROM integration.attribution_jobs AS job
         WHERE (
                 (job.processing_status = 'pending' AND job.available_at <= claimed_at)
                 OR (job.processing_status = 'processing' AND job.locked_at <= stale_before)
               )
           AND job.attempts < 8
         ORDER BY job.available_at, job.created_at, job.commerce_fact_id
         FOR UPDATE OF job SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.attribution_jobs AS job
       SET processing_status = 'processing',
           attempts = job.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at,
           updated_at = claimed_at
      FROM claimable
     WHERE job.store_id = claimable.store_id
       AND job.commerce_fact_id = claimable.commerce_fact_id
       AND job.model_version = claimable.model_version
    RETURNING job.commerce_fact_id, job.store_id,
              job.model_version, job.attempts;
$$;

CREATE FUNCTION integration.claim_export_deliveries(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    destination_id UUID,
    commerce_fact_id UUID,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.export_deliveries AS delivery
           SET delivery_status = 'dead_letter', locked_by = NULL, locked_at = NULL,
               last_error = COALESCE(
                   delivery.last_error,
                   'worker lease expired after final attempt'
               ), updated_at = claimed_at
         WHERE delivery.delivery_status = 'processing'
           AND delivery.locked_at <= stale_before AND delivery.attempts >= 8
        RETURNING delivery.id
    ), claimable AS (
        SELECT delivery.id
          FROM integration.export_deliveries AS delivery
          INNER JOIN integration.destination_accounts AS destination
            ON destination.store_id = delivery.store_id
           AND destination.id = delivery.destination_id
         WHERE (
                 (delivery.delivery_status = 'pending'
                    AND delivery.available_at <= claimed_at)
                 OR (delivery.delivery_status = 'processing'
                    AND delivery.locked_at <= stale_before)
               )
           AND destination.enabled
           AND delivery.attempts < 8
         ORDER BY delivery.available_at, delivery.created_at, delivery.id
         FOR UPDATE OF delivery SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.export_deliveries AS delivery
       SET delivery_status = 'processing', attempts = delivery.attempts + 1,
           locked_by = worker_id, locked_at = claimed_at, updated_at = claimed_at
      FROM claimable
     WHERE delivery.id = claimable.id
    RETURNING delivery.id, delivery.store_id,
              delivery.destination_id, delivery.commerce_fact_id, delivery.attempts;
$$;

CREATE FUNCTION integration.rebuild_store_attribution(
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
    IF requested_store_id <>
       nullif(current_setting('app.store_id', true), '')::uuid THEN
        RETURN 0;
    END IF;
    IF requested_model_version <= 0 THEN
        RETURN 0;
    END IF;

    DELETE FROM integration.attribution_results AS result
     WHERE result.store_id = requested_store_id
       AND result.model_version = requested_model_version;

    UPDATE integration.attribution_jobs AS job
       SET processing_status = 'pending',
           attempts = 0,
           available_at = requested_at,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = NULL,
           last_error = NULL,
           updated_at = requested_at
     WHERE job.store_id = requested_store_id
       AND job.model_version = requested_model_version;
    GET DIAGNOSTICS rebuilt = ROW_COUNT;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION integration.claim_sessionization_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    behavior_event_id UUID,
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
          FROM integration.behavior_event_processing AS job
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
        UPDATE integration.behavior_event_processing AS job
           SET processing_status = 'processing',
               locked_by = worker_id,
               locked_at = claimed_at,
               attempts = least(job.attempts, 30) + 1,
               updated_at = claimed_at
          FROM claimable
         WHERE job.id = claimable.id
        RETURNING job.id, job.store_id, job.attempts
    )
    SELECT event.id,
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
      INNER JOIN integration.behavior_events AS event
        ON event.store_id = claimed.store_id
       AND event.id = claimed.id
     ORDER BY event.received_at, event.id;
$$;

ALTER TABLE integration.behavior_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.store_policy_versions ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.identity_links ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.behavior_event_processing ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.sessions ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.erasure_requests ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.commerce_facts ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.attribution_jobs ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.attribution_results ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.daily_behavior_reports ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.daily_commerce_reports ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.daily_attribution_reports ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.destination_accounts ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.export_deliveries ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.behavior_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.store_policy_versions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.identity_links
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.behavior_event_processing
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.sessions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.erasure_requests
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.commerce_facts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.attribution_jobs
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.attribution_results
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.daily_behavior_reports
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.daily_commerce_reports
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.daily_attribution_reports
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.destination_accounts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.export_deliveries
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION integration.claim_commerce_fact_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_attribution_jobs(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_export_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.rebuild_store_attribution(
    UUID, SMALLINT, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_sessionization_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.sessionization_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.attribution_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.export_delivery_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.retention_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.apply_store_retention_policy(UUID, INTEGER) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.purge_expired_data(INTEGER, TIMESTAMPTZ) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.process_erasure_requests(INTEGER, TIMESTAMPTZ) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.erasure_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_commerce_fact_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_attribution_jobs(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_export_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.rebuild_store_attribution(
    UUID, SMALLINT, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_sessionization_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.sessionization_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.attribution_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.export_delivery_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.retention_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.apply_store_retention_policy(UUID, INTEGER)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.purge_expired_data(INTEGER, TIMESTAMPTZ)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.process_erasure_requests(INTEGER, TIMESTAMPTZ)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.erasure_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE UPDATE, DELETE ON integration.store_policy_versions FROM chaos_runtime;

REVOKE UPDATE, DELETE ON integration.identity_links FROM chaos_runtime;

REVOKE UPDATE, DELETE ON integration.behavior_events FROM chaos_runtime;

REVOKE UPDATE, DELETE ON integration.erasure_requests FROM chaos_runtime;

REVOKE UPDATE, DELETE ON integration.commerce_facts FROM chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;

-- === Cross-schema reliability constraints ===

ALTER TABLE commerce.order_fulfillment_transitions
    ADD CONSTRAINT order_fulfillment_transitions_source_event_id_fkey
    FOREIGN KEY (source_event_id) REFERENCES integration.outbox_events(id);
