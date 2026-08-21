-- === Integration capability slimming ===
--
-- Notification delivery is intentionally out of scope for the current
-- runtime. Provider dashboards remain the source of truth until the
-- notification capability is rebuilt as a separate feature.

SELECT pgmq.create('chaos_analytics_destinations');

-- These queues only served the capabilities removed by this migration. Their
-- messages are intentionally discarded together with the removed delivery
-- records; the new destination queue is the only Analytics delivery queue.
SELECT pgmq.drop_queue('chaos_email');
SELECT pgmq.drop_queue('chaos_meta');

DROP FUNCTION IF EXISTS commerce.resolve_notification_webhook(UUID);
DROP TABLE IF EXISTS commerce.notification_provider_accounts CASCADE;

DROP FUNCTION IF EXISTS integration.record_resend_webhook(
    UUID, TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
);
DROP FUNCTION IF EXISTS integration.finish_email_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
);
DROP FUNCTION IF EXISTS integration.claim_email_deliveries(INTEGER);
DROP FUNCTION IF EXISTS integration.enqueue_email_delivery();
DROP TABLE IF EXISTS integration.webhook_events CASCADE;
DROP TABLE IF EXISTS integration.email_suppressions CASCADE;
DROP TABLE IF EXISTS integration.email_deliveries CASCADE;
DROP TYPE IF EXISTS integration.email_suppression_reason;
DROP TYPE IF EXISTS integration.email_delivery_status;

DROP INDEX IF EXISTS integration.provider_metric_snapshots_query_idx;
DROP TABLE IF EXISTS integration.provider_metric_snapshots CASCADE;
DROP FUNCTION IF EXISTS integration.process_analytics_erasure_requests(INTEGER, TIMESTAMPTZ);
DROP TABLE IF EXISTS integration.analytics_erasure_requests CASCADE;
DROP TYPE IF EXISTS integration.erasure_status;

DROP FUNCTION IF EXISTS integration.finish_meta_delivery(
    UUID, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
);
DROP FUNCTION IF EXISTS integration.claim_meta_event_deliveries(INTEGER);
DROP FUNCTION IF EXISTS integration.enqueue_meta_delivery();
DROP TABLE IF EXISTS integration.meta_event_deliveries CASCADE;
DROP TABLE IF EXISTS integration.meta_connections CASCADE;

ALTER TABLE integration.analytics_settings
    RENAME COLUMN meta_reporting_enabled TO provider_reporting_enabled;

ALTER TABLE integration.commerce_events
    RENAME COLUMN meta_eligible TO provider_eligible;
ALTER TABLE integration.commerce_events
    RENAME CONSTRAINT commerce_events_meta_eligibility_check
        TO commerce_events_provider_eligibility_check;

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
    FOREIGN KEY (store_id, commerce_event_id)
        REFERENCES integration.commerce_events(store_id, id) ON DELETE CASCADE,
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
