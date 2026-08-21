-- === Reliable delivery foundation ===

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
SELECT pgmq.create('chaos_email');
SELECT pgmq.create('chaos_meta');

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
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    provider                   TEXT        NOT NULL,
    provider_event_id          TEXT        NOT NULL,
    event_type                 TEXT        NOT NULL,
    external_account_reference TEXT        NOT NULL,
    payload                    JSONB       NOT NULL,
    pgmq_message_id            BIGINT      NOT NULL UNIQUE,
    processed_at               TIMESTAMPTZ,
    failed_at                  TIMESTAMPTZ,
    last_error                 TEXT,
    verified_at                TIMESTAMPTZ NOT NULL,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider, provider_event_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
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

-- === Notifications ===

CREATE TYPE integration.email_delivery_status AS ENUM (
    'pending',
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
    provider_account_id      UUID,
    provider_message_id      TEXT,
    delivery_status          integration.email_delivery_status  NOT NULL DEFAULT 'pending',
    delivery_attempts        INTEGER                            NOT NULL DEFAULT 0,
    pgmq_message_id          BIGINT                             NOT NULL UNIQUE,
    sent_at                  TIMESTAMPTZ,
    delivered_at             TIMESTAMPTZ,
    last_error               TEXT,
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, semantic_event_id),
    UNIQUE (provider_account_id, provider_message_id),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.notification_provider_accounts(store_id, id),
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
    CONSTRAINT email_deliveries_attempts_check CHECK (delivery_attempts >= 0),
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
    suppression_reason    integration.email_suppression_reason      NOT NULL,
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
    provider_account_id   UUID                     NOT NULL,
    provider              TEXT                     NOT NULL,
    provider_event_id     TEXT                     NOT NULL,
    provider_event_type   TEXT                     NOT NULL,
    payload               JSONB                    NOT NULL,
    received_at           TIMESTAMPTZ              NOT NULL,
    processed_at          TIMESTAMPTZ,
    created_at            TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider_account_id, provider_event_id),
    FOREIGN KEY (store_id, delivery_id)
        REFERENCES integration.email_deliveries(store_id, id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.notification_provider_accounts(store_id, id),
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
    ON integration.email_deliveries (created_at, id)
    WHERE delivery_status = 'pending';

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

CREATE FUNCTION integration.enqueue_email_delivery()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          'chaos_email',
          jsonb_build_object('version', 1, 'delivery_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.claim_email_deliveries(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    recipient_email TEXT,
    template_key TEXT,
    template_version INTEGER,
    template_payload JSONB,
    provider TEXT,
    provider_account_id UUID,
    credential_secret_reference TEXT,
    sender TEXT,
    attempts INTEGER
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    message RECORD;
    delivery RECORD;
BEGIN
    FOR message IN
        SELECT *
          FROM pgmq.read(
              'chaos_email',
              120,
              greatest(least(batch_size, 100), 1),
              '{}'::jsonb
          )
    LOOP
        SELECT candidate.*
          INTO delivery
          FROM integration.email_deliveries AS candidate
         WHERE candidate.pgmq_message_id = message.msg_id
           AND candidate.delivery_status = 'pending';
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_email', message.msg_id);
            CONTINUE;
        END IF;
        IF EXISTS (
            SELECT 1
              FROM integration.email_suppressions AS suppression
             WHERE suppression.store_id = delivery.store_id
               AND suppression.recipient_email = delivery.recipient_email
        ) THEN
            UPDATE integration.email_deliveries AS candidate
               SET delivery_status = 'suppressed',
                   last_error = 'recipient is suppressed',
                   updated_at = CURRENT_TIMESTAMP
             WHERE candidate.id = delivery.id;
            PERFORM pgmq.delete('chaos_email', message.msg_id);
            CONTINUE;
        END IF;
        SELECT account.id, account.credential_secret_reference, account.sender
          INTO provider_account_id, credential_secret_reference, sender
          FROM commerce.notification_provider_accounts AS account
         WHERE account.store_id = delivery.store_id
           AND account.provider = delivery.provider
           AND account.enabled;
        IF NOT FOUND THEN
            CONTINUE;
        END IF;
        UPDATE integration.email_deliveries AS candidate
           SET provider_account_id = claim_email_deliveries.provider_account_id,
               delivery_attempts = candidate.delivery_attempts + 1,
               updated_at = CURRENT_TIMESTAMP
         WHERE candidate.id = delivery.id;
        id := delivery.id;
        store_id := delivery.store_id;
        recipient_email := delivery.recipient_email::text;
        template_key := delivery.template_key;
        template_version := delivery.template_version;
        template_payload := delivery.template_payload;
        provider := delivery.provider;
        attempts := delivery.delivery_attempts + 1;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.finish_email_delivery(
    delivery_id UUID,
    attempts INTEGER,
    succeeded BOOLEAN,
    retryable BOOLEAN,
    provider_message_id TEXT,
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
      FROM integration.email_deliveries AS delivery
     WHERE delivery.id = delivery_id
       AND delivery.delivery_status = 'pending'
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR NOT retryable OR attempts >= 8 THEN
        UPDATE integration.email_deliveries AS delivery
           SET delivery_status = CASE
                   WHEN succeeded THEN 'sent'::integration.email_delivery_status
                   WHEN NOT retryable THEN 'failed'::integration.email_delivery_status
                   ELSE 'dead_letter'::integration.email_delivery_status
               END,
               provider_message_id = CASE
                   WHEN succeeded THEN finish_email_delivery.provider_message_id
                   ELSE delivery.provider_message_id
               END,
               sent_at = CASE WHEN succeeded THEN finished_at ELSE delivery.sent_at END,
               template_payload = CASE
                   WHEN succeeded THEN '{}'::jsonb ELSE delivery.template_payload
               END,
               last_error = CASE
                   WHEN succeeded THEN NULL
                   ELSE COALESCE(NULLIF(left(failure, 2000), ''), 'email delivery failed')
               END,
               updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.delete('chaos_email', message_id);
    ELSE
        UPDATE integration.email_deliveries AS delivery
           SET last_error = COALESCE(NULLIF(left(failure, 2000), ''), 'email delivery failed'),
               updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.set_vt(
            'chaos_email',
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

CREATE FUNCTION integration.record_resend_webhook(
    provider_account_id UUID,
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
       AND delivery.provider_account_id = record_resend_webhook.provider_account_id
       AND delivery.provider_message_id = record_resend_webhook.provider_message_id
     FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    webhook_id := uuidv7();
    INSERT INTO integration.webhook_events (
        id, store_id, delivery_id, provider_account_id, provider, provider_event_id,
        provider_event_type, payload, received_at, processed_at
    ) VALUES (
        webhook_id, target.store_id, target.id, record_resend_webhook.provider_account_id, 'resend',
        record_resend_webhook.provider_event_id, record_resend_webhook.provider_event_type,
        payload, received_at, received_at
    ) ON CONFLICT ON CONSTRAINT webhook_events_provider_account_id_provider_event_id_key DO NOTHING;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    IF provider_event_type = 'email.sent'
       AND target.delivery_status IN ('pending', 'sent') THEN
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

CREATE TRIGGER email_deliveries_enqueue
BEFORE INSERT ON integration.email_deliveries
FOR EACH ROW EXECUTE FUNCTION integration.enqueue_email_delivery();

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
    INTEGER
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_email_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.enqueue_email_delivery() FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.record_resend_webhook(
    UUID, TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_email_deliveries(
    INTEGER
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_email_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.record_resend_webhook(
    UUID, TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.email_deliveries, integration.email_suppressions,
       integration.webhook_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;

-- === Analytics ===

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

CREATE TYPE integration.erasure_status AS ENUM ('pending', 'completed');

CREATE TABLE integration.analytics_settings (
    store_id                    UUID        NOT NULL PRIMARY KEY,
    revision                    INTEGER     NOT NULL,
    collection_enabled          BOOLEAN     NOT NULL,
    browser_collection_mode     integration.browser_collection_mode NOT NULL,
    meta_reporting_enabled      BOOLEAN     NOT NULL,
    identity_linking_enabled    BOOLEAN     NOT NULL,
    raw_event_retention_days    SMALLINT    NOT NULL,
    updated_by                  UUID        NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,

    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT analytics_settings_revision_check CHECK (revision > 0),
    CONSTRAINT analytics_settings_retention_check CHECK (
        raw_event_retention_days BETWEEN 1 AND 400
    )
);

CREATE TABLE integration.visitor_customer_links (
    id                    UUID        NOT NULL PRIMARY KEY,
    store_id              UUID        NOT NULL,
    visitor_id            UUID        NOT NULL,
    customer_id           UUID        NOT NULL,
    consent_policy_version TEXT       NOT NULL,
    advertising_storage_consent BOOLEAN NOT NULL,
    collection_basis      integration.browser_collection_basis NOT NULL,
    settings_revision     INTEGER     NOT NULL,
    linked_at             TIMESTAMPTZ NOT NULL,
    retention_expires_at  TIMESTAMPTZ NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, visitor_id, customer_id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, customer_id) REFERENCES commerce.customers(store_id, id) ON DELETE CASCADE,
    CONSTRAINT visitor_customer_links_visitor_check CHECK (
        visitor_id <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT visitor_customer_links_policy_check CHECK (
        consent_policy_version ~ '^[A-Za-z0-9_.:-]{1,64}$'
    ),
    CONSTRAINT visitor_customer_links_basis_check CHECK (
        collection_basis IN ('consent', 'store_policy')
    ),
    CONSTRAINT visitor_customer_links_revision_check CHECK (settings_revision > 0),
    CONSTRAINT visitor_customer_links_retention_check CHECK (
        retention_expires_at > linked_at
    )
);

CREATE TABLE integration.commerce_events (
    id                          UUID                            NOT NULL PRIMARY KEY,
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
    meta_eligible               BOOLEAN                         NOT NULL,
    consent_policy_version      TEXT,
    settings_revision           INTEGER                         NOT NULL,
    properties                  JSONB                            NOT NULL DEFAULT '{}'::jsonb,
    occurred_at                 TIMESTAMPTZ                      NOT NULL,
    received_at                 TIMESTAMPTZ                      NOT NULL,
    retention_expires_at        TIMESTAMPTZ                      NOT NULL,
    created_at                  TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, event_id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, sales_channel_id) REFERENCES commerce.sales_channels(store_id, id),
    FOREIGN KEY (store_id, customer_id) REFERENCES commerce.customers(store_id, id),
    FOREIGN KEY (store_id, product_id) REFERENCES commerce.products(store_id, id),
    FOREIGN KEY (store_id, product_variant_id) REFERENCES commerce.product_variants(store_id, id),
    FOREIGN KEY (store_id, cart_id) REFERENCES commerce.carts(store_id, id),
    FOREIGN KEY (store_id, checkout_id) REFERENCES commerce.checkouts(store_id, id),
    FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    FOREIGN KEY (store_id, payment_attempt_id) REFERENCES commerce.payment_attempts(store_id, id),
    FOREIGN KEY (store_id, refund_id) REFERENCES commerce.refunds(store_id, id),
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
    CONSTRAINT commerce_events_meta_eligibility_check CHECK (
        NOT meta_eligible
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
    ),
    CONSTRAINT commerce_events_retention_check CHECK (
        retention_expires_at > received_at
    )
);

CREATE TABLE integration.meta_connections (
    store_id                    UUID        NOT NULL PRIMARY KEY,
    dataset_id                  TEXT        NOT NULL,
    credential_secret_reference TEXT       NOT NULL,
    test_event_code             TEXT,
    capi_enabled                BOOLEAN     NOT NULL,
    created_by                  UUID        NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,

    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT meta_connections_dataset_check CHECK (dataset_id ~ '^[0-9]{5,32}$'),
    CONSTRAINT meta_connections_secret_check CHECK (
        credential_secret_reference ~ '^(enc://[A-Za-z0-9_-]+|env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96})$'
        AND octet_length(credential_secret_reference) <= 518
    ),
    CONSTRAINT meta_connections_test_code_check CHECK (
        test_event_code IS NULL OR octet_length(test_event_code) BETWEEN 1 AND 64
    )
);

CREATE TABLE integration.meta_event_deliveries (
    id                  UUID                     NOT NULL PRIMARY KEY,
    store_id            UUID                     NOT NULL,
    commerce_event_id   UUID                     NOT NULL,
    delivery_status     integration.delivery_status NOT NULL DEFAULT 'pending',
    pgmq_message_id     BIGINT                   NOT NULL UNIQUE,
    delivered_at        TIMESTAMPTZ,
    provider_reference  TEXT,
    last_error          TEXT,
    created_at          TIMESTAMPTZ              NOT NULL,
    updated_at          TIMESTAMPTZ              NOT NULL,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    UNIQUE (store_id, commerce_event_id),
    FOREIGN KEY (store_id, commerce_event_id)
        REFERENCES integration.commerce_events(store_id, id) ON DELETE CASCADE,
    CONSTRAINT meta_event_deliveries_completion_check CHECK (
        (delivery_status = 'processed' AND delivered_at IS NOT NULL)
        OR (delivery_status <> 'processed' AND delivered_at IS NULL)
    ),
    CONSTRAINT meta_event_deliveries_reference_check CHECK (
        provider_reference IS NULL OR octet_length(provider_reference) <= 512
    ),
    CONSTRAINT meta_event_deliveries_error_check CHECK (
        last_error IS NULL OR octet_length(last_error) <= 2048
    )
);

CREATE TABLE integration.provider_metric_snapshots (
    id                         UUID          NOT NULL PRIMARY KEY,
    store_id                   UUID          NOT NULL,
    provider                   TEXT          NOT NULL,
    external_account_reference TEXT          NOT NULL,
    metric_date                DATE          NOT NULL,
    metric_name                TEXT          NOT NULL,
    dimensions                 JSONB         NOT NULL DEFAULT '{}'::jsonb,
    value_numeric              NUMERIC(30, 6) NOT NULL,
    currency                   CHAR(3),
    source_reference           TEXT,
    observed_at                TIMESTAMPTZ   NOT NULL,
    raw_snapshot               JSONB         NOT NULL DEFAULT '{}'::jsonb,
    created_at                 TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT provider_metric_snapshots_provider_check CHECK (
        provider ~ '^[a-z][a-z0-9_]{1,31}$'
    ),
    CONSTRAINT provider_metric_snapshots_account_check CHECK (
        octet_length(external_account_reference) BETWEEN 1 AND 255
    ),
    CONSTRAINT provider_metric_snapshots_metric_check CHECK (
        metric_name ~ '^[a-z][a-z0-9_]{1,63}$'
    ),
    CONSTRAINT provider_metric_snapshots_dimensions_check CHECK (
        jsonb_typeof(dimensions) = 'object' AND octet_length(dimensions::text) <= 8192
    ),
    CONSTRAINT provider_metric_snapshots_currency_check CHECK (
        currency IS NULL OR currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT provider_metric_snapshots_source_check CHECK (
        source_reference IS NULL OR octet_length(source_reference) <= 512
    ),
    CONSTRAINT provider_metric_snapshots_raw_check CHECK (
        jsonb_typeof(raw_snapshot) = 'object' AND octet_length(raw_snapshot::text) <= 32768
    )
);

CREATE TABLE integration.analytics_erasure_requests (
    id                       UUID                       NOT NULL PRIMARY KEY,
    store_id                 UUID                       NOT NULL,
    visitor_id               UUID,
    customer_id              UUID,
    status                   integration.erasure_status NOT NULL DEFAULT 'pending',
    requested_by             UUID                       NOT NULL,
    commerce_events_deleted  BIGINT                     NOT NULL DEFAULT 0,
    visitor_links_deleted    BIGINT                     NOT NULL DEFAULT 0,
    requested_at             TIMESTAMPTZ                NOT NULL,
    completed_at             TIMESTAMPTZ,
    created_at               TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, customer_id) REFERENCES commerce.customers(store_id, id),
    CONSTRAINT analytics_erasure_requests_selector_check CHECK (
        (visitor_id IS NOT NULL)::integer + (customer_id IS NOT NULL)::integer = 1
    ),
    CONSTRAINT analytics_erasure_requests_counts_check CHECK (
        commerce_events_deleted >= 0 AND visitor_links_deleted >= 0
    ),
    CONSTRAINT analytics_erasure_requests_completion_check CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status = 'pending' AND completed_at IS NULL)
    )
);

CREATE INDEX commerce_events_visitor_path_idx
    ON integration.commerce_events (store_id, visitor_id, occurred_at, id)
    WHERE visitor_id IS NOT NULL;

CREATE INDEX commerce_events_customer_path_idx
    ON integration.commerce_events (store_id, customer_id, occurred_at, id)
    WHERE customer_id IS NOT NULL;

CREATE INDEX commerce_events_channel_time_idx
    ON integration.commerce_events (store_id, sales_channel_id, occurred_at DESC, id DESC);

CREATE INDEX commerce_events_retention_idx
    ON integration.commerce_events (retention_expires_at, id);

CREATE INDEX visitor_customer_links_customer_idx
    ON integration.visitor_customer_links (store_id, customer_id, linked_at DESC);

CREATE INDEX visitor_customer_links_retention_idx
    ON integration.visitor_customer_links (retention_expires_at, id);

CREATE INDEX meta_event_deliveries_claim_idx
    ON integration.meta_event_deliveries (created_at, id)
    WHERE delivery_status = 'pending';

CREATE INDEX provider_metric_snapshots_query_idx
    ON integration.provider_metric_snapshots (
        store_id, provider, metric_date DESC, metric_name
    );

CREATE INDEX analytics_erasure_requests_pending_idx
    ON integration.analytics_erasure_requests (requested_at, id)
    WHERE status = 'pending';

CREATE FUNCTION integration.claim_meta_event_deliveries(
    batch_size INTEGER
)
RETURNS TABLE (id UUID, store_id UUID, commerce_event_id UUID, attempts INTEGER)
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
                   'chaos_meta',
                   120,
                   greatest(least(batch_size, 100), 1),
                   '{}'::jsonb
               ) AS queued
    LOOP
        SELECT delivery.id, delivery.store_id, delivery.commerce_event_id
          INTO target
          FROM integration.meta_event_deliveries AS delivery
         WHERE delivery.pgmq_message_id = message.msg_id
           AND delivery.delivery_status = 'pending';
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_meta', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        commerce_event_id := target.commerce_event_id;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.enqueue_meta_delivery()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    SELECT message_id
      INTO NEW.pgmq_message_id
      FROM pgmq.send(
          'chaos_meta',
          jsonb_build_object('version', 1, 'delivery_id', NEW.id)
      ) AS message_id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.finish_meta_delivery(
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
      FROM integration.meta_event_deliveries AS delivery
     WHERE delivery.id = delivery_id
       AND delivery.delivery_status = 'pending'
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR NOT retryable OR attempts >= 8 THEN
        UPDATE integration.meta_event_deliveries AS delivery
           SET delivery_status = CASE
                   WHEN succeeded THEN 'processed'::integration.delivery_status
                   ELSE 'dead_letter'::integration.delivery_status
               END,
               delivered_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               provider_reference = finish_meta_delivery.provider_reference,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2048) END,
               updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.delete('chaos_meta', message_id);
    ELSE
        UPDATE integration.meta_event_deliveries AS delivery
           SET last_error = left(failure, 2048), updated_at = finished_at
         WHERE delivery.id = delivery_id;
        PERFORM pgmq.set_vt(
            'chaos_meta',
            message_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
    RETURN true;
END;
$$;

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

CREATE FUNCTION integration.purge_expired_analytics_data(
    batch_size INTEGER,
    purged_at TIMESTAMPTZ
)
RETURNS TABLE (commerce_events_deleted BIGINT, visitor_links_deleted BIGINT)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, integration
AS $$
DECLARE
    events_count BIGINT;
    links_count BIGINT;
BEGIN
    WITH expired AS (
        SELECT event.id FROM integration.commerce_events AS event
         WHERE event.retention_expires_at <= purged_at
         ORDER BY event.retention_expires_at, event.id
         FOR UPDATE SKIP LOCKED LIMIT batch_size
    )
    DELETE FROM integration.commerce_events AS event USING expired
     WHERE event.id = expired.id;
    GET DIAGNOSTICS events_count = ROW_COUNT;

    WITH expired AS (
        SELECT link.id FROM integration.visitor_customer_links AS link
         WHERE link.retention_expires_at <= purged_at
         ORDER BY link.retention_expires_at, link.id
         FOR UPDATE SKIP LOCKED LIMIT batch_size
    )
    DELETE FROM integration.visitor_customer_links AS link USING expired
     WHERE link.id = expired.id;
    GET DIAGNOSTICS links_count = ROW_COUNT;

    RETURN QUERY SELECT events_count, links_count;
END;
$$;

CREATE FUNCTION integration.process_analytics_erasure_requests(
    batch_size INTEGER,
    processed_at TIMESTAMPTZ
)
RETURNS TABLE (
    requests_completed BIGINT,
    commerce_events_deleted BIGINT,
    visitor_links_deleted BIGINT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, integration
AS $$
DECLARE
    request_row RECORD;
    events_count BIGINT := 0;
    links_count BIGINT := 0;
    deleted_count BIGINT;
    deleted_links_count BIGINT;
    completed_count BIGINT := 0;
BEGIN
    FOR request_row IN
        SELECT request.id, request.store_id, request.visitor_id, request.customer_id
          FROM integration.analytics_erasure_requests AS request
         WHERE request.status = 'pending'
         ORDER BY request.requested_at, request.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    LOOP
        DELETE FROM integration.commerce_events AS event
         WHERE event.store_id = request_row.store_id
           AND ((request_row.visitor_id IS NOT NULL AND event.visitor_id = request_row.visitor_id)
             OR (request_row.customer_id IS NOT NULL AND event.customer_id = request_row.customer_id));
        GET DIAGNOSTICS deleted_count = ROW_COUNT;
        events_count := events_count + deleted_count;

        DELETE FROM integration.visitor_customer_links AS link
         WHERE link.store_id = request_row.store_id
           AND ((request_row.visitor_id IS NOT NULL AND link.visitor_id = request_row.visitor_id)
             OR (request_row.customer_id IS NOT NULL AND link.customer_id = request_row.customer_id));
        GET DIAGNOSTICS deleted_links_count = ROW_COUNT;
        links_count := links_count + deleted_links_count;

        UPDATE integration.analytics_erasure_requests
           SET status = 'completed', commerce_events_deleted = deleted_count,
               visitor_links_deleted = deleted_links_count, completed_at = processed_at,
               updated_at = processed_at
         WHERE id = request_row.id;
        completed_count := completed_count + 1;
    END LOOP;
    RETURN QUERY SELECT completed_count, events_count, links_count;
END;
$$;

CREATE TRIGGER meta_event_deliveries_enqueue
BEFORE INSERT ON integration.meta_event_deliveries
FOR EACH ROW EXECUTE FUNCTION integration.enqueue_meta_delivery();

ALTER TABLE integration.analytics_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_settings FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.visitor_customer_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.visitor_customer_links FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.commerce_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.commerce_events FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.meta_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.meta_connections FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.meta_event_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.meta_event_deliveries FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.provider_metric_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.provider_metric_snapshots FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_erasure_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_erasure_requests FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_settings
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.visitor_customer_links
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.commerce_events
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.meta_connections
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.meta_event_deliveries
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.provider_metric_snapshots
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);
CREATE POLICY store_isolation ON integration.analytics_erasure_requests
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.claim_meta_event_deliveries(
    INTEGER
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_meta_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_meta_delivery() FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_analytics_events(
    INTEGER
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.purge_expired_analytics_data(
    INTEGER, TIMESTAMPTZ
) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.process_analytics_erasure_requests(
    INTEGER, TIMESTAMPTZ
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION integration.claim_meta_event_deliveries(
    INTEGER
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_meta_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_analytics_events(
    INTEGER
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.purge_expired_analytics_data(
    INTEGER, TIMESTAMPTZ
) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.process_analytics_erasure_requests(
    INTEGER, TIMESTAMPTZ
) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;
REVOKE UPDATE, DELETE ON integration.commerce_events FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.visitor_customer_links FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.provider_metric_snapshots FROM chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;

-- === Cross-schema reliability constraints ===

ALTER TABLE commerce.order_fulfillment_transitions
    ADD CONSTRAINT order_fulfillment_transitions_source_event_id_fkey
    FOREIGN KEY (source_event_id) REFERENCES integration.outbox_events(id);
