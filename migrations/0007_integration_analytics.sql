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
    created_at                  TIMESTAMPTZ     NOT NULL,
    updated_at                  TIMESTAMPTZ     NOT NULL,

    CONSTRAINT analytics_destinations_store_id_id_key        UNIQUE (store_id, id),
    CONSTRAINT analytics_destinations_store_id_provider_key  UNIQUE (store_id, provider),
    CONSTRAINT analytics_destinations_store_id_fkey          FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT analytics_destinations_provider_check         CHECK (provider ~ '^[a-z][a-z0-9_]{1,31}$'),
    CONSTRAINT analytics_destinations_account_check          CHECK (octet_length(external_account_reference) BETWEEN 1 AND 255),
    CONSTRAINT analytics_destinations_secret_check           CHECK (credential_secret_reference ~ '^(enc://[A-Za-z0-9_-]+|env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96})$' AND octet_length(credential_secret_reference) <= 518),
    CONSTRAINT analytics_destinations_configuration_check    CHECK (jsonb_typeof(configuration) = 'object' AND octet_length(configuration::text) <= 16384)
);

CREATE FUNCTION integration.configure_analytics_destination (
    p_store_id                       UUID,
    p_provider                       TEXT,
    p_external_account_reference     TEXT,
    p_credential_secret_reference    TEXT,
    p_configuration                  JSONB,
    p_enabled                        BOOLEAN,
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

    RETURN QUERY
    INSERT INTO integration.analytics_destinations (
        id,
        store_id,
        provider,
        external_account_reference,
        credential_secret_reference,
        configuration,
        enabled,
        created_at,
        updated_at
    )
    VALUES (
        uuidv7(),
        p_store_id,
        p_provider,
        p_external_account_reference,
        p_credential_secret_reference,
        p_configuration,
        p_enabled,
        p_now,
        p_now
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

ALTER TABLE integration.analytics_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_events FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_destinations ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_destinations FORCE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.analytics_event_keys FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.analytics_events
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_destinations
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.analytics_event_keys
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.configure_analytics_destination (UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, TIMESTAMPTZ) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.configure_analytics_destination (UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, TIMESTAMPTZ) TO chaos_runtime;

SELECT cron.schedule(
    'chaos-analytics-partition-maintenance',
    '5 0 * * *',
    'SELECT partman.run_maintenance();'
);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_event_keys
    TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.analytics_events,
       integration.analytics_destinations,
       integration.analytics_event_keys
    FROM chaos_runtime;

REVOKE INSERT
    ON integration.analytics_destinations
    FROM chaos_runtime;
