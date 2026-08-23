CREATE SCHEMA integration;

CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'store', 'shopper');

SELECT pgmq.create('chaos_shipping_events');
SELECT pgmq.create('chaos_search_events');

CREATE TABLE integration.idempotency_keys (
    id                   UUID                             NOT NULL PRIMARY KEY,
    scope                integration.idempotency_scope    NOT NULL,
    scope_id             UUID                             NOT NULL,
    operation            TEXT                             NOT NULL,
    idempotency_key      TEXT                             NOT NULL,
    request_fingerprint  BYTEA                            NOT NULL,
    response_status      SMALLINT,
    response_body        JSONB,
    completed_at         TIMESTAMPTZ,
    expires_at           TIMESTAMPTZ                      NOT NULL DEFAULT (CURRENT_TIMESTAMP + INTERVAL '7 days'),
    created_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT idempotency_keys_scope_scope_id_operation_key_key    UNIQUE (scope, scope_id, operation, idempotency_key),
    CONSTRAINT idempotency_keys_operation_length_check              CHECK (length(operation) BETWEEN 1 AND 120),
    CONSTRAINT idempotency_keys_key_length_check                    CHECK (octet_length(idempotency_key) BETWEEN 1 AND 255),
    CONSTRAINT idempotency_keys_request_fingerprint_length_check    CHECK (octet_length(request_fingerprint) = 32),
    CONSTRAINT idempotency_keys_response_completion_check           CHECK ((response_status IS NULL AND response_body IS NULL AND completed_at IS NULL) OR (response_status BETWEEN 200 AND 599 AND response_body IS NOT NULL AND completed_at IS NOT NULL))
);

CREATE INDEX idempotency_keys_expiry_idx ON integration.idempotency_keys (expires_at) WHERE completed_at IS NOT NULL;

SELECT cron.schedule(
    'chaos-idempotency-cleanup',
    '17 * * * *',
    $$
    DELETE FROM integration.idempotency_keys
     WHERE id IN (
         SELECT id
           FROM integration.idempotency_keys
          WHERE completed_at IS NOT NULL
            AND expires_at < CURRENT_TIMESTAMP
          ORDER BY expires_at, id
          LIMIT 10000
     )
    $$
);

CREATE TABLE integration.event_consumers (
    event_type  TEXT PRIMARY KEY,
    queue_name  TEXT NOT NULL,
    description TEXT NOT NULL,
    CONSTRAINT event_consumers_event_type_check      CHECK (event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    CONSTRAINT event_consumers_queue_name_check      CHECK (queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_consumers_description_check     CHECK (length(trim(description)) BETWEEN 1 AND 255)
);

CREATE TABLE integration.event_outbox (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    aggregate_type       TEXT        NOT NULL,
    aggregate_id         UUID        NOT NULL,
    event_type           TEXT        NOT NULL,
    payload              JSONB       NOT NULL,
    queue_name           TEXT        NOT NULL,
    pgmq_message_id      BIGINT      NOT NULL,
    processed_at         TIMESTAMPTZ,
    failed_at            TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT event_outbox_queue_name_pgmq_message_id_key    UNIQUE (queue_name, pgmq_message_id),
    CONSTRAINT event_outbox_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT event_outbox_event_type_fkey                   FOREIGN KEY (event_type) REFERENCES integration.event_consumers(event_type),
    CONSTRAINT event_outbox_payload_object_check              CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT event_outbox_queue_name_check                  CHECK (queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_outbox_completion_check                  CHECK (processed_at IS NULL OR failed_at IS NULL)
);

CREATE INDEX event_outbox_pending_idx ON integration.event_outbox (created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;

CREATE FUNCTION integration.event_queue_name(event_type TEXT)
RETURNS TEXT
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT registry.queue_name
      FROM integration.event_consumers AS registry
     WHERE registry.event_type = event_queue_name.event_type;
$$;

CREATE FUNCTION integration.enqueue_event_outbox()
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
    NEW.queue_name := queue_name;
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          queue_name,
          jsonb_build_object('version', 1, 'event_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.claim_routed_event_outbox(
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
        'chaos_shipping_events',
        'chaos_search_events'
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
          FROM integration.event_outbox AS event
         WHERE event.queue_name = $1
           AND event.pgmq_message_id = message.msg_id
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

CREATE FUNCTION integration.claim_shipping_events(
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
               'chaos_shipping_events', batch_size
           ) AS event;
$$;

CREATE FUNCTION integration.finish_event_outbox(
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
           event.queue_name
      INTO message_id, queue_name
      FROM integration.event_outbox AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL OR queue_name IS NULL THEN
        RETURN false;
    END IF;

    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE integration.event_outbox AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete(queue_name, message_id);
    ELSE
        UPDATE integration.event_outbox AS event
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

ALTER TABLE integration.idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.event_outbox ENABLE ROW LEVEL SECURITY;

CREATE POLICY idempotency_scope_isolation ON integration.idempotency_keys
    USING ((scope = 'user' AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid) OR (scope = 'store' AND scope_id = nullif(current_setting('app.store_id', true), '')::uuid) OR (scope = 'shopper' AND scope_id = nullif(current_setting('app.shopper_id', true), '')::uuid))
    WITH CHECK ((scope = 'user' AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid) OR (scope = 'store' AND scope_id = nullif(current_setting('app.store_id', true), '')::uuid) OR (scope = 'shopper' AND scope_id = nullif(current_setting('app.shopper_id', true), '')::uuid));

CREATE POLICY store_isolation ON integration.event_outbox
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

INSERT INTO integration.event_consumers (event_type, queue_name, description)
VALUES
    ('search.product.changed', 'chaos_search_events', 'Refreshes the Store-isolated Product search document'),
    ('shipping.shipped', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback'),
    ('shipping.delivered', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback'),
    ('shipping.cancelled', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback');

CREATE TRIGGER event_outbox_enqueue
    BEFORE INSERT ON integration.event_outbox
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_event_outbox();

REVOKE ALL ON FUNCTION integration.event_queue_name(TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_event_outbox() FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_routed_event_outbox(TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_shipping_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_event_outbox(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_shipping_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_event_outbox(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA integration TO chaos_runtime;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON integration.event_consumers FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.event_outbox FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;


REVOKE CREATE ON SCHEMA public FROM PUBLIC;
