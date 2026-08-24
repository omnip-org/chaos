CREATE TYPE integration.delivery_status AS ENUM ('pending', 'processed', 'dead_letter');

SELECT pgmq.create('chaos_analytics_deliveries');

CREATE TABLE integration.analytics_events (
    id                  UUID         NOT NULL,
    event_id            UUID         NOT NULL,
    store_id            UUID         NOT NULL,
    shopper_id          UUID         NOT NULL,
    event_name          TEXT         NOT NULL,
    properties          JSONB        NOT NULL DEFAULT '{}'::jsonb,
    occurred_at         TIMESTAMPTZ  NOT NULL,
    received_at         TIMESTAMPTZ  NOT NULL,
    created_at          TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

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
    max_attempts INTEGER,
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
    IF succeeded OR NOT retryable OR attempts >= greatest(max_attempts, 1) THEN
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
REVOKE ALL ON FUNCTION integration.finish_analytics_event_delivery(UUID, INTEGER, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_analytics_event_delivery() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.configure_analytics_destination(UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_analytics_deliveries(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_analytics_event_delivery(UUID, INTEGER, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) TO chaos_runtime;

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

GRANT SELECT, INSERT, UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_deliveries
    TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_deliveries
    FROM chaos_runtime;
