CREATE TYPE integration.delivery_status AS ENUM ('pending', 'processed', 'dead_letter');

SELECT pgmq.create('chaos_analytics_deliveries');

CREATE TABLE integration.analytics_events (
    id             UUID           NOT NULL,
    event_id       UUID           NOT NULL,
    store_id       UUID           NOT NULL,
    channel_id     UUID           NOT NULL,
    shopper_id     UUID           NOT NULL,
    session_id     UUID,
    utm_source     TEXT,
    utm_medium     TEXT,
    utm_campaign   TEXT,
    utm_term       TEXT,
    utm_content    TEXT,
    event_name     TEXT           NOT NULL,
    event_source   TEXT           NOT NULL,
    properties     JSONB          NOT NULL DEFAULT '{}'::jsonb,
    occurred_at    TIMESTAMPTZ    NOT NULL,
    received_at    TIMESTAMPTZ    NOT NULL,
    created_at     TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT analytics_events_received_id_pkey               PRIMARY KEY (received_at, id),
    CONSTRAINT analytics_events_store_received_event_key       UNIQUE (store_id, received_at, event_id),
    CONSTRAINT analytics_events_store_received_id_key          UNIQUE (store_id, received_at, id),
    CONSTRAINT analytics_events_store_id_channel_id_fkey       FOREIGN KEY (store_id, channel_id) REFERENCES commerce.channels (store_id, id) ON DELETE CASCADE,
    CONSTRAINT analytics_events_store_id_shopper_id_fkey       FOREIGN KEY (store_id, shopper_id) REFERENCES commerce.shoppers (store_id, id) ON DELETE CASCADE,
    CONSTRAINT analytics_events_event_id_check                 CHECK (event_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT analytics_events_event_name_check               CHECK (event_name ~ '^[a-z][a-z0-9_]{0,63}$'),
    CONSTRAINT analytics_events_event_source_check             CHECK (event_source IN ('browser', 'server')),
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
CREATE INDEX analytics_events_channel_time_idx ON integration.analytics_events (store_id, channel_id, occurred_at DESC, id DESC);
CREATE INDEX analytics_events_name_time_idx ON integration.analytics_events (store_id, event_name, occurred_at DESC, id DESC);
CREATE INDEX analytics_events_event_key_idx ON integration.analytics_events (store_id, event_id);
CREATE INDEX analytics_events_source_idx ON integration.analytics_events (store_id, event_source, received_at DESC, id DESC);
CREATE INDEX analytics_events_session_idx ON integration.analytics_events (store_id, session_id, occurred_at DESC, id DESC) WHERE session_id IS NOT NULL;
CREATE INDEX analytics_events_utm_source_idx ON integration.analytics_events (store_id, utm_source, occurred_at DESC, id DESC) WHERE utm_source IS NOT NULL;
CREATE INDEX analytics_events_utm_medium_idx ON integration.analytics_events (store_id, utm_medium, occurred_at DESC, id DESC) WHERE utm_medium IS NOT NULL;
CREATE INDEX analytics_events_utm_campaign_idx ON integration.analytics_events (store_id, utm_campaign, occurred_at DESC, id DESC) WHERE utm_campaign IS NOT NULL;
CREATE INDEX analytics_events_utm_term_idx ON integration.analytics_events (store_id, utm_term, occurred_at DESC, id DESC) WHERE utm_term IS NOT NULL;
CREATE INDEX analytics_events_utm_content_idx ON integration.analytics_events (store_id, utm_content, occurred_at DESC, id DESC) WHERE utm_content IS NOT NULL;
CREATE INDEX analytics_events_checkout_order_idx ON integration.analytics_events (store_id, (properties->>'order_id'), occurred_at DESC, received_at DESC, id DESC) WHERE event_name = 'initiate_checkout' AND event_source = 'browser' AND properties ? 'order_id';

CREATE TABLE integration.analytics_event_keys (
    store_id                    UUID          NOT NULL,
    event_name                  TEXT          NOT NULL,
    event_id                    UUID          NOT NULL,
    event_received_at           TIMESTAMPTZ   NOT NULL,
    analytics_event_id          UUID          NOT NULL,
    created_at                  TIMESTAMPTZ   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT analytics_event_keys_pkey              PRIMARY KEY (store_id, event_name, event_id),
    CONSTRAINT analytics_event_keys_event_id_check    CHECK (event_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT analytics_event_keys_event_name_check  CHECK (event_name ~ '^[a-z][a-z0-9_]{0,63}$'),
    CONSTRAINT analytics_event_keys_store_id_fkey     FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT analytics_event_keys_event_fkey        FOREIGN KEY (store_id, event_received_at, analytics_event_id) REFERENCES integration.analytics_events (store_id, received_at, id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX analytics_event_keys_event_idx ON integration.analytics_event_keys (store_id, event_received_at, analytics_event_id);

CREATE TABLE integration.analytics_destinations (
    id                          UUID            NOT NULL PRIMARY KEY,
    store_id                    UUID            NOT NULL,
    provider                    TEXT            NOT NULL,
    external_account_reference  TEXT            NOT NULL,
    credential_secret_reference TEXT            NOT NULL,
    configuration               JSONB           NOT NULL DEFAULT '{}'::jsonb,
    enabled                     BOOLEAN         NOT NULL,
    created_by                  UUID            NOT NULL,
    created_at                  TIMESTAMPTZ     NOT NULL,
    updated_at                  TIMESTAMPTZ     NOT NULL,
    schedule_cursor_received_at TIMESTAMPTZ     NOT NULL,
    schedule_cursor_event_id    UUID            NOT NULL,

    CONSTRAINT analytics_destinations_store_id_id_key        UNIQUE (store_id, id),
    CONSTRAINT analytics_destinations_store_id_provider_key  UNIQUE (store_id, provider),
    CONSTRAINT analytics_destinations_store_id_fkey          FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT analytics_destinations_provider_check         CHECK (provider ~ '^[a-z][a-z0-9_]{1,31}$'),
    CONSTRAINT analytics_destinations_account_check          CHECK (octet_length(external_account_reference) BETWEEN 1 AND 255),
    CONSTRAINT analytics_destinations_secret_check           CHECK (credential_secret_reference ~ '^(enc://[A-Za-z0-9_-]+|env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96})$' AND octet_length(credential_secret_reference) <= 518),
    CONSTRAINT analytics_destinations_configuration_check    CHECK (jsonb_typeof(configuration) = 'object' AND octet_length(configuration::text) <= 16384)
);

CREATE TABLE integration.analytics_deliveries (
    id                          UUID                          NOT NULL PRIMARY KEY,
    store_id                    UUID                          NOT NULL,
    destination_id              UUID                          NOT NULL,
    analytics_event_received_at TIMESTAMPTZ                   NOT NULL,
    analytics_event_id          UUID                          NOT NULL,
    delivery_status             integration.delivery_status   NOT NULL DEFAULT 'pending',
    pgmq_message_id             BIGINT                        UNIQUE,
    delivered_at                TIMESTAMPTZ,
    provider_reference          TEXT,
    last_error                  TEXT,
    created_at                  TIMESTAMPTZ                   NOT NULL,
    updated_at                  TIMESTAMPTZ                   NOT NULL,

    CONSTRAINT analytics_deliveries_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT analytics_deliveries_store_id_destination_event_key    UNIQUE (store_id, destination_id, analytics_event_received_at, analytics_event_id),
    CONSTRAINT analytics_deliveries_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT analytics_deliveries_store_id_destination_fkey         FOREIGN KEY (store_id, destination_id) REFERENCES integration.analytics_destinations (store_id, id) ON DELETE CASCADE,
    CONSTRAINT analytics_deliveries_store_event_fkey                  FOREIGN KEY (store_id, analytics_event_received_at, analytics_event_id) REFERENCES integration.analytics_events (store_id, received_at, id) ON DELETE CASCADE,
    CONSTRAINT analytics_deliveries_completion_check                  CHECK ((delivery_status = 'processed' AND delivered_at IS NOT NULL) OR (delivery_status <> 'processed' AND delivered_at IS NULL)),
    CONSTRAINT analytics_deliveries_reference_check                   CHECK (provider_reference IS NULL OR octet_length(provider_reference) <= 512),
    CONSTRAINT analytics_deliveries_error_check                       CHECK (last_error IS NULL OR octet_length(last_error) <= 2048)
);

CREATE INDEX analytics_deliveries_claim_idx ON integration.analytics_deliveries (created_at, id) WHERE delivery_status = 'pending';
CREATE INDEX analytics_deliveries_event_idx ON integration.analytics_deliveries (store_id, analytics_event_received_at, analytics_event_id, destination_id, delivery_status);
CREATE INDEX analytics_destinations_schedule_idx ON integration.analytics_destinations (store_id, id) WHERE enabled;

CREATE FUNCTION integration.configure_analytics_destination (
    p_store_id                       UUID,
    p_provider                       TEXT,
    p_external_account_reference     TEXT,
    p_credential_secret_reference    TEXT,
    p_configuration                  JSONB,
    p_enabled                        BOOLEAN,
    p_created_by                     UUID,
    p_now                            TIMESTAMPTZ
)
RETURNS TABLE (
    destination_id                         UUID,
    destination_provider                   TEXT,
    destination_external_account_reference TEXT,
    destination_configuration              JSONB,
    destination_enabled                    BOOLEAN,
    destination_created_at                 TIMESTAMPTZ,
    destination_updated_at                 TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF p_now IS NULL THEN
        RAISE EXCEPTION 'analytics destination time is required'
            USING ERRCODE = '22023';
    END IF;

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
        id,
        store_id,
        provider,
        external_account_reference,
        credential_secret_reference,
        configuration,
        enabled,
        created_by,
        created_at,
        updated_at,
        schedule_cursor_received_at,
        schedule_cursor_event_id
    )
    SELECT
        uuidv7(),
        p_store_id,
        p_provider,
        p_external_account_reference,
        p_credential_secret_reference,
        p_configuration,
        p_enabled,
        p_created_by,
        p_now,
        p_now,
        CASE
            WHEN latest_event.received_at IS NULL OR latest_event.received_at < p_now
            THEN p_now
            ELSE latest_event.received_at
        END,
        CASE
            WHEN latest_event.received_at IS NULL OR latest_event.received_at < p_now
            THEN activation.activation_id
            WHEN latest_event.received_at = p_now
                 AND latest_event.id < activation.activation_id
            THEN activation.activation_id
            ELSE latest_event.id
        END
    FROM (SELECT uuidv7() AS activation_id) AS activation
    LEFT JOIN LATERAL (
        SELECT event.received_at, event.id
        FROM integration.analytics_events AS event
        WHERE event.store_id = p_store_id
          AND event.received_at <= p_now
        ORDER BY event.received_at DESC, event.id DESC
        LIMIT 1
    ) AS latest_event ON true
    ON CONFLICT (store_id, provider) DO UPDATE SET
        external_account_reference = EXCLUDED.external_account_reference,
        credential_secret_reference = EXCLUDED.credential_secret_reference,
        configuration = EXCLUDED.configuration,
        enabled = EXCLUDED.enabled,
        schedule_cursor_received_at = CASE
            WHEN analytics_destinations.enabled IS DISTINCT FROM EXCLUDED.enabled
            THEN EXCLUDED.schedule_cursor_received_at
            ELSE analytics_destinations.schedule_cursor_received_at
        END,
        schedule_cursor_event_id = CASE
            WHEN analytics_destinations.enabled IS DISTINCT FROM EXCLUDED.enabled
            THEN EXCLUDED.schedule_cursor_event_id
            ELSE analytics_destinations.schedule_cursor_event_id
        END,
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

CREATE FUNCTION integration.claim_analytics_deliveries (batch_size INTEGER)
RETURNS TABLE (
    id                 UUID,
    store_id           UUID,
    destination_id     UUID,
    analytics_event_id UUID,
    attempts           INTEGER
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    message RECORD;
    target  RECORD;
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
        SELECT
            delivery.id,
            delivery.store_id,
            delivery.destination_id,
            delivery.analytics_event_id
        INTO target
        FROM integration.analytics_deliveries AS delivery
        WHERE delivery.pgmq_message_id = message.msg_id
          AND delivery.delivery_status = 'pending';

        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_analytics_deliveries', message.msg_id);
            CONTINUE;
        END IF;

        id                 := target.id;
        store_id           := target.store_id;
        destination_id     := target.destination_id;
        analytics_event_id := target.analytics_event_id;
        attempts           := message.read_ct;

        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.enqueue_analytics_event_delivery ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    queued_message_id BIGINT;
BEGIN
    SELECT message_id
    INTO queued_message_id
    FROM pgmq.send(
        'chaos_analytics_deliveries',
        jsonb_build_object('version', 1, 'delivery_id', NEW.id)
    ) AS message_id;

    IF queued_message_id IS NULL THEN
        RAISE EXCEPTION 'analytics delivery queue did not return a message identifier';
    END IF;

    UPDATE integration.analytics_deliveries
    SET pgmq_message_id = queued_message_id
    WHERE id = NEW.id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION integration.finish_analytics_event_delivery (
    delivery_id        UUID,
    attempts           INTEGER,
    max_attempts       INTEGER,
    succeeded          BOOLEAN,
    retryable          BOOLEAN,
    provider_reference TEXT,
    failure            TEXT,
    finished_at        TIMESTAMPTZ
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
        SET delivery_status    = CASE WHEN succeeded THEN 'processed'::integration.delivery_status ELSE 'dead_letter'::integration.delivery_status END,
            delivered_at       = CASE WHEN succeeded THEN finished_at ELSE NULL END,
            provider_reference = finish_analytics_event_delivery.provider_reference,
            last_error         = CASE WHEN succeeded THEN NULL ELSE left(failure, 2048) END,
            updated_at         = finished_at
        WHERE delivery.id = delivery_id;

        PERFORM pgmq.delete('chaos_analytics_deliveries', message_id);
    ELSE
        UPDATE integration.analytics_deliveries AS delivery
        SET last_error = left(failure, 2048),
            updated_at = finished_at
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

CREATE FUNCTION integration.schedule_analytics_deliveries (batch_size INTEGER)
RETURNS BIGINT
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    destination      RECORD;
    event_row        RECORD;
    scheduled        BIGINT := 0;
    inserted_count   INTEGER;
    last_received_at TIMESTAMPTZ;
    last_event_id    UUID;
BEGIN
    IF batch_size IS NULL OR batch_size NOT BETWEEN 1 AND 100 THEN
        RAISE EXCEPTION 'batch_size must be between 1 and 100'
            USING ERRCODE = '22023';
    END IF;

    FOR destination IN
        SELECT
            target.id,
            target.store_id,
            target.schedule_cursor_received_at,
            target.schedule_cursor_event_id
        FROM integration.analytics_destinations AS target
        WHERE target.enabled
        ORDER BY target.store_id, target.id
        FOR UPDATE SKIP LOCKED
    LOOP
        EXIT WHEN scheduled >= batch_size;
        last_received_at := NULL;
        last_event_id := NULL;

        FOR event_row IN
            SELECT event_item.received_at, event_item.id
            FROM integration.analytics_events AS event_item
            WHERE event_item.store_id = destination.store_id
              AND (event_item.received_at, event_item.id) > (
                  destination.schedule_cursor_received_at,
                  destination.schedule_cursor_event_id
              )
            ORDER BY event_item.received_at, event_item.id
            LIMIT (batch_size - scheduled)
        LOOP
            INSERT INTO integration.analytics_deliveries (
                id,
                store_id,
                destination_id,
                analytics_event_received_at,
                analytics_event_id,
                created_at,
                updated_at
            )
            VALUES (
                uuidv7(),
                destination.store_id,
                destination.id,
                event_row.received_at,
                event_row.id,
                CURRENT_TIMESTAMP,
                CURRENT_TIMESTAMP
            )
            ON CONFLICT (store_id, destination_id, analytics_event_received_at, analytics_event_id)
                DO NOTHING;

            GET DIAGNOSTICS inserted_count = ROW_COUNT;
            scheduled := scheduled + inserted_count;
            last_received_at := event_row.received_at;
            last_event_id := event_row.id;
            EXIT WHEN scheduled >= batch_size;
        END LOOP;

        IF last_received_at IS NOT NULL AND last_event_id IS NOT NULL THEN
            UPDATE integration.analytics_destinations
            SET schedule_cursor_received_at = last_received_at,
                schedule_cursor_event_id = last_event_id,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = destination.id;
        END IF;
    END LOOP;

    RETURN scheduled;
END;
$$;

CREATE TRIGGER analytics_deliveries_enqueue
    AFTER INSERT ON integration.analytics_deliveries
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_analytics_event_delivery();

ALTER TABLE integration.analytics_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_events FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_destinations ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_destinations FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_deliveries FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_keys FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_events
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_destinations
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_deliveries
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_event_keys
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.configure_analytics_destination (UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_analytics_deliveries (INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_analytics_event_delivery (UUID, INTEGER, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_analytics_event_delivery () FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.schedule_analytics_deliveries (INTEGER) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.configure_analytics_destination (UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_analytics_deliveries (INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_analytics_event_delivery (UUID, INTEGER, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.schedule_analytics_deliveries (INTEGER) TO chaos_runtime;

SELECT cron.schedule(
    'chaos-analytics-partition-maintenance',
    '5 0 * * *',
    'SELECT partman.run_maintenance();'
);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_deliveries,
       integration.analytics_event_keys
    TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_deliveries,
       integration.analytics_event_keys
    FROM chaos_runtime;

REVOKE INSERT
    ON integration.analytics_destinations,
       integration.analytics_deliveries
    FROM chaos_runtime;
