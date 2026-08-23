CREATE SCHEMA integration;

CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'store', 'shopper');
CREATE TYPE integration.delivery_status AS ENUM ('pending', 'processed', 'dead_letter');

SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_shipping_events');
SELECT pgmq.create('chaos_search_events');
SELECT pgmq.create('chaos_webhooks');
SELECT pgmq.create('chaos_analytics_deliveries');

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

CREATE TABLE integration.provider_webhooks (
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
    CONSTRAINT provider_webhooks_provider_account_id_provider_event_id_key    UNIQUE (provider_account_id, provider_event_id),
    CONSTRAINT provider_webhooks_store_id_fkey                                FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT provider_webhooks_store_id_provider_account_fkey               FOREIGN KEY (store_id, provider_account_id) REFERENCES commerce.payment_provider_accounts(store_id, id),
    CONSTRAINT provider_webhooks_payload_object_check                         CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT provider_webhooks_completion_check                             CHECK (processed_at IS NULL OR failed_at IS NULL)
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

CREATE INDEX provider_webhooks_claim_idx ON integration.provider_webhooks (created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;
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

CREATE FUNCTION integration.claim_event_outbox(
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
          FROM integration.provider_webhooks AS event
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
      FROM integration.provider_webhooks AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE integration.provider_webhooks AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSE
        UPDATE integration.provider_webhooks AS event
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

ALTER TABLE integration.idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.provider_webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.event_outbox ENABLE ROW LEVEL SECURITY;

CREATE POLICY idempotency_scope_isolation ON integration.idempotency_keys
    USING ((scope = 'user' AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid) OR (scope = 'store' AND scope_id = nullif(current_setting('app.store_id', true), '')::uuid) OR (scope = 'shopper' AND scope_id = nullif(current_setting('app.shopper_id', true), '')::uuid))
    WITH CHECK ((scope = 'user' AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid) OR (scope = 'store' AND scope_id = nullif(current_setting('app.store_id', true), '')::uuid) OR (scope = 'shopper' AND scope_id = nullif(current_setting('app.shopper_id', true), '')::uuid));

CREATE POLICY store_isolation ON integration.provider_webhooks
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.event_outbox
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

INSERT INTO integration.event_consumers (event_type, queue_name, description)
VALUES
    ('payment.create_requested', 'chaos_payment_commands', 'Creates a Stripe Checkout Session for the Order'),
    ('refund.create_requested', 'chaos_payment_commands', 'Creates a Stripe Refund for the Order'),
    ('search.product.changed', 'chaos_search_events', 'Refreshes the Store-isolated Product search document'),
    ('shipping.shipped', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback'),
    ('shipping.delivered', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback'),
    ('shipping.cancelled', 'chaos_shipping_events', 'Updates the Order shipping state from a provider callback');

CREATE TRIGGER event_outbox_enqueue
    BEFORE INSERT ON integration.event_outbox
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_event_outbox();

CREATE TRIGGER provider_webhooks_enqueue
    BEFORE INSERT ON integration.provider_webhooks
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_webhook_event();

REVOKE ALL ON FUNCTION integration.event_queue_name(TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_event_outbox() FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_webhook_event() FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_routed_event_outbox(TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_event_outbox(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_shipping_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_webhook_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_event_outbox(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_event_outbox(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_shipping_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_webhook_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_event_outbox(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA integration TO chaos_runtime;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON integration.event_consumers FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.provider_webhooks, integration.event_outbox FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;

CREATE INDEX provider_webhooks_provider_account_idx ON integration.provider_webhooks (provider_account_id, created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;

-- ============================================================
-- Analytics workflow
-- ============================================================

CREATE TABLE integration.analytics_events (
    id                  UUID        NOT NULL,
    event_id            UUID        NOT NULL,
    store_id            UUID        NOT NULL,
    shopper_id          UUID        NOT NULL,
    event_name          TEXT        NOT NULL,
    properties          JSONB       NOT NULL DEFAULT '{}'::jsonb,
    occurred_at         TIMESTAMPTZ NOT NULL,
    received_at         TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT analytics_events_received_id_pkey               PRIMARY KEY (received_at, id),
    CONSTRAINT analytics_events_store_received_event_key       UNIQUE (store_id, received_at, event_id),
    CONSTRAINT analytics_events_event_name_check               CHECK (event_name ~ '^[a-z][a-z0-9_]{0,63}$'),
    CONSTRAINT analytics_events_properties_check               CHECK (jsonb_typeof(properties) = 'object' AND octet_length(properties::text) <= 32768),
    CONSTRAINT analytics_events_time_check                     CHECK (occurred_at >= received_at - INTERVAL '24 hours' AND occurred_at <= received_at + INTERVAL '5 minutes')
) PARTITION BY RANGE (received_at);

SELECT partman.create_partition(
    p_parent_table := 'integration.analytics_events',
    p_control := 'received_at',
    p_interval := '1 day',
    p_premake := 7,
    p_default_table := true,
    p_automatic_maintenance := 'on',
    p_jobmon := false
);

CREATE INDEX analytics_events_shopper_path_idx ON integration.analytics_events (store_id, shopper_id, occurred_at, id);
CREATE INDEX analytics_events_name_time_idx ON integration.analytics_events (store_id, event_name, occurred_at DESC, id DESC);
CREATE INDEX analytics_events_event_key_idx ON integration.analytics_events (store_id, event_id);
CREATE INDEX analytics_events_store_id_idx ON integration.analytics_events (store_id, id DESC);

ALTER TABLE integration.analytics_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_events FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_events
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT ON integration.analytics_events TO chaos_runtime;
REVOKE UPDATE, DELETE ON integration.analytics_events FROM chaos_runtime;

CREATE TABLE integration.analytics_destinations (
    id                          UUID        NOT NULL PRIMARY KEY,
    store_id                    UUID        NOT NULL,
    provider                    TEXT        NOT NULL,
    external_account_reference  TEXT        NOT NULL,
    credential_secret_reference TEXT        NOT NULL,
    configuration               JSONB       NOT NULL DEFAULT '{}'::jsonb,
    enabled                     BOOLEAN     NOT NULL,
    created_by                  UUID        NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,
    CONSTRAINT analytics_destinations_store_id_id_key                UNIQUE (store_id, id),
    CONSTRAINT analytics_destinations_store_id_provider_key          UNIQUE (store_id, provider),
    CONSTRAINT analytics_destinations_store_id_fkey                  FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT analytics_destinations_provider_check                 CHECK (provider ~ '^[a-z][a-z0-9_]{1,31}$'),
    CONSTRAINT analytics_destinations_account_check                  CHECK (octet_length(external_account_reference) BETWEEN 1 AND 255),
    CONSTRAINT analytics_destinations_secret_check                   CHECK (credential_secret_reference ~ '^(enc://[A-Za-z0-9_-]+|env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96})$' AND octet_length(credential_secret_reference) <= 518),
    CONSTRAINT analytics_destinations_configuration_check            CHECK (jsonb_typeof(configuration) = 'object' AND octet_length(configuration::text) <= 16384)
);

CREATE FUNCTION integration.configure_analytics_destination(
    p_store_id UUID,
    p_provider TEXT,
    p_external_account_reference TEXT,
    p_credential_secret_reference TEXT,
    p_configuration JSONB,
    p_enabled BOOLEAN,
    p_created_by UUID,
    p_now TIMESTAMPTZ
)
RETURNS TABLE (
    destination_id UUID,
    destination_provider TEXT,
    destination_external_account_reference TEXT,
    destination_configuration JSONB,
    destination_enabled BOOLEAN,
    destination_created_at TIMESTAMPTZ,
    destination_updated_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF p_store_id IS DISTINCT FROM nullif(current_setting('app.store_id', true), '')::uuid THEN
        RAISE EXCEPTION 'analytics destination store context does not match target store'
            USING ERRCODE = '42501';
    END IF;

    IF p_created_by IS DISTINCT FROM nullif(current_setting('app.user_id', true), '')::uuid THEN
        RAISE EXCEPTION 'analytics destination user context does not match creator'
            USING ERRCODE = '42501';
    END IF;

    RETURN QUERY
    INSERT INTO integration.analytics_destinations (
        id, store_id, provider, external_account_reference,
        credential_secret_reference, configuration, enabled, created_by, created_at, updated_at
    )
    VALUES (
        uuidv7(), p_store_id, p_provider, p_external_account_reference,
        p_credential_secret_reference, p_configuration, p_enabled, p_created_by, p_now, p_now
    )
    ON CONFLICT (store_id, provider) DO UPDATE SET
        external_account_reference = EXCLUDED.external_account_reference,
        credential_secret_reference = EXCLUDED.credential_secret_reference,
        configuration = EXCLUDED.configuration,
        enabled = EXCLUDED.enabled,
        updated_at = EXCLUDED.updated_at
    RETURNING
        analytics_destinations.id,
        analytics_destinations.provider,
        analytics_destinations.external_account_reference,
        analytics_destinations.configuration,
        analytics_destinations.enabled,
        analytics_destinations.created_at,
        analytics_destinations.updated_at;
END;
$$;

REVOKE ALL ON FUNCTION integration.configure_analytics_destination(UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ) FROM PUBLIC;

CREATE TABLE integration.analytics_deliveries (
    id                  UUID                        NOT NULL PRIMARY KEY,
    store_id            UUID                        NOT NULL,
    destination_id      UUID                        NOT NULL,
    analytics_event_id  UUID                        NOT NULL,
    delivery_status     integration.delivery_status NOT NULL DEFAULT 'pending',
    pgmq_message_id     BIGINT                      NOT NULL UNIQUE,
    delivered_at        TIMESTAMPTZ,
    provider_reference  TEXT,
    last_error          TEXT,
    created_at          TIMESTAMPTZ                 NOT NULL,
    updated_at          TIMESTAMPTZ                 NOT NULL,
    CONSTRAINT analytics_deliveries_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT analytics_deliveries_store_id_destination_event_key    UNIQUE (store_id, destination_id, analytics_event_id),
    CONSTRAINT analytics_deliveries_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT analytics_deliveries_store_id_destination_fkey         FOREIGN KEY (store_id, destination_id) REFERENCES integration.analytics_destinations(store_id, id) ON DELETE CASCADE,
    CONSTRAINT analytics_deliveries_completion_check                  CHECK ((delivery_status = 'processed' AND delivered_at IS NOT NULL) OR (delivery_status <> 'processed' AND delivered_at IS NULL)),
    CONSTRAINT analytics_deliveries_reference_check                   CHECK (provider_reference IS NULL OR octet_length(provider_reference) <= 512),
    CONSTRAINT analytics_deliveries_error_check                       CHECK (last_error IS NULL OR octet_length(last_error) <= 2048)
);

CREATE INDEX analytics_deliveries_claim_idx ON integration.analytics_deliveries (created_at, id) WHERE delivery_status = 'pending';

CREATE FUNCTION integration.claim_analytics_deliveries(
    batch_size INTEGER
)
RETURNS TABLE (id UUID, store_id UUID, destination_id UUID, analytics_event_id UUID, attempts INTEGER)
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
                   'chaos_analytics_deliveries',
                   120,
                   greatest(least(batch_size, 100), 1),
                   '{}'::jsonb
               ) AS queued
    LOOP
        SELECT delivery.id, delivery.store_id, delivery.destination_id,
               delivery.analytics_event_id
          INTO target
          FROM integration.analytics_deliveries AS delivery
         WHERE delivery.pgmq_message_id = message.msg_id
           AND delivery.delivery_status = 'pending';
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_analytics_deliveries', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        destination_id := target.destination_id;
        analytics_event_id := target.analytics_event_id;
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
          'chaos_analytics_deliveries',
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
      FROM integration.analytics_deliveries AS delivery
     WHERE delivery.id = delivery_id
       AND delivery.delivery_status = 'pending'
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR NOT retryable OR attempts >= 8 THEN
        UPDATE integration.analytics_deliveries AS delivery
           SET delivery_status = CASE WHEN succeeded THEN 'processed'::integration.delivery_status ELSE 'dead_letter'::integration.delivery_status END,
               delivered_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               provider_reference = finish_analytics_event_delivery.provider_reference,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2048) END,
               updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.delete('chaos_analytics_deliveries', message_id);
    ELSE
        UPDATE integration.analytics_deliveries AS delivery
           SET last_error = left(failure, 2048), updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.set_vt(
            'chaos_analytics_deliveries',
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

CREATE TRIGGER analytics_deliveries_enqueue
    BEFORE INSERT ON integration.analytics_deliveries
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_analytics_event_delivery();

ALTER TABLE integration.analytics_destinations ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_destinations FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_deliveries FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_destinations
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_deliveries
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.claim_analytics_deliveries(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_analytics_event_delivery(UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_analytics_event_delivery() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.configure_analytics_destination(UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_analytics_deliveries(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_analytics_event_delivery(UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) TO chaos_runtime;

REVOKE UPDATE, DELETE ON integration.analytics_destinations, integration.analytics_deliveries FROM chaos_runtime;

CREATE FUNCTION integration.schedule_analytics_deliveries(
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
               event.id AS analytics_event_id,
               destination.id AS destination_id
          FROM integration.analytics_events AS event
         JOIN integration.analytics_destinations AS destination
            ON destination.store_id = event.store_id
           AND destination.enabled
         WHERE NOT EXISTS (
               SELECT 1
                 FROM integration.analytics_deliveries AS delivery
                WHERE delivery.store_id = event.store_id
                  AND delivery.destination_id = destination.id
                  AND delivery.analytics_event_id = event.id
           )
         ORDER BY event.received_at, event.id, destination.id
         LIMIT greatest(least(batch_size, 100), 0)
    )
    INSERT INTO integration.analytics_deliveries (
        id, store_id, destination_id, analytics_event_id, created_at, updated_at
    )
    SELECT uuidv7(), store_id, destination_id, analytics_event_id,
           CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
      FROM candidates
    ON CONFLICT (store_id, destination_id, analytics_event_id) DO NOTHING;

    GET DIAGNOSTICS scheduled = ROW_COUNT;
    RETURN scheduled;
END;
$$;

REVOKE ALL ON FUNCTION integration.schedule_analytics_deliveries(INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION integration.schedule_analytics_deliveries(INTEGER) TO chaos_runtime;

SELECT cron.schedule(
    'chaos-analytics-partition-maintenance',
    '5 0 * * *',
    'SELECT partman.run_maintenance();'
);

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON integration.event_consumers FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.provider_webhooks, integration.event_outbox, integration.analytics_events, integration.analytics_deliveries FROM chaos_runtime;
