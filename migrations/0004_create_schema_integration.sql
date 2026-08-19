CREATE SCHEMA integration;

COMMENT ON SCHEMA integration IS
    'Idempotency records, webhooks, outbox delivery, and external mappings';

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
        REFERENCES merchant.stores(id),
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
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
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
