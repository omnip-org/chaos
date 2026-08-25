CREATE SCHEMA integration;

SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_email_commands');
SELECT pgmq.create('chaos_shipping_commands');
SELECT pgmq.create('chaos_search_events');
SELECT pgmq.create('chaos_webhooks');

CREATE TYPE integration.provider_capability AS ENUM ('email', 'payment', 'shipping');
CREATE TYPE integration.webhook_processing_status AS ENUM ('pending', 'processed', 'unsupported', 'failed');

CREATE TABLE integration.event_routes (
    internal_event_type TEXT PRIMARY KEY,
    queue_name          TEXT NOT NULL,
    description         TEXT NOT NULL,

    CONSTRAINT event_routes_internal_event_type_format_check CHECK (internal_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    CONSTRAINT event_routes_queue_name_format_check          CHECK (queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_routes_description_length_check         CHECK (length(trim(description)) BETWEEN 1 AND 255)
);

CREATE TABLE integration.event_outbox (
    id                  UUID        NOT NULL PRIMARY KEY,
    store_id            UUID        NOT NULL,
    aggregate_type      TEXT        NOT NULL,
    aggregate_id        UUID        NOT NULL,
    internal_event_type TEXT        NOT NULL,
    payload             JSONB       NOT NULL,
    queue_name          TEXT,
    pgmq_message_id     BIGINT      UNIQUE,
    processed_at        TIMESTAMPTZ,
    failed_at           TIMESTAMPTZ,
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT event_outbox_queue_name_pgmq_message_id_key      UNIQUE (queue_name, pgmq_message_id),
    CONSTRAINT event_outbox_store_id_fkey                       FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT event_outbox_internal_event_type_fkey            FOREIGN KEY (internal_event_type) REFERENCES integration.event_routes (internal_event_type),
    CONSTRAINT event_outbox_aggregate_type_format_check         CHECK (aggregate_type ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT event_outbox_payload_object_check                CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT event_outbox_queue_name_format_check             CHECK (queue_name IS NULL OR queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_outbox_dispatch_pair_check                 CHECK ((queue_name IS NULL) = (pgmq_message_id IS NULL)),
    CONSTRAINT event_outbox_completion_check                    CHECK (processed_at IS NULL OR failed_at IS NULL),
    CONSTRAINT event_outbox_last_error_length_check             CHECK (last_error IS NULL OR length(last_error) <= 2000)
);

CREATE TABLE integration.provider_accounts (
    id                           UUID                            NOT NULL PRIMARY KEY,
    store_id                     UUID                            NOT NULL,
    capability                   integration.provider_capability NOT NULL,
    provider                     TEXT                            NOT NULL,
    display_name                 TEXT                            NOT NULL DEFAULT 'Integration Provider',
    credential_secret_reference  TEXT,
    webhook_secret_reference     TEXT,
    configuration                JSONB                           NOT NULL DEFAULT '{}'::jsonb,
    enabled                      BOOLEAN                         NOT NULL DEFAULT true,
    created_at                   TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT provider_accounts_store_capability_provider_key     UNIQUE (store_id, capability, provider),
    CONSTRAINT provider_accounts_store_id_id_key                   UNIQUE (store_id, id),
    CONSTRAINT provider_accounts_store_id_capability_provider_key  UNIQUE (store_id, id, capability, provider),
    CONSTRAINT provider_accounts_store_id_fkey                     FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT provider_accounts_provider_format_check             CHECK (provider ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT provider_accounts_display_name_length_check         CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT provider_accounts_credential_reference_check        CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT provider_accounts_webhook_reference_check           CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT provider_accounts_configuration_object_check        CHECK (jsonb_typeof(configuration) = 'object'),
    CONSTRAINT provider_accounts_configuration_size_check          CHECK (pg_column_size(configuration) <= 32768)
);

CREATE TABLE integration.provider_webhook_inbox (
    id                    UUID                                  NOT NULL PRIMARY KEY,
    store_id              UUID                                  NOT NULL,
    provider_account_id   UUID                                  NOT NULL,
    capability            integration.provider_capability       NOT NULL,
    provider              TEXT                                  NOT NULL,
    provider_event_id     TEXT                                  NOT NULL,
    provider_event_type   TEXT                                  NOT NULL,
    normalized_event_type TEXT,
    payload               JSONB                                 NOT NULL,
    aggregate_type        TEXT,
    aggregate_id          UUID,
    pgmq_message_id       BIGINT                                UNIQUE,
    processing_status     integration.webhook_processing_status NOT NULL DEFAULT 'pending',
    processed_at          TIMESTAMPTZ,
    unsupported_at        TIMESTAMPTZ,
    failed_at             TIMESTAMPTZ,
    last_error            TEXT,
    verified_at           TIMESTAMPTZ                           NOT NULL,
    received_at           TIMESTAMPTZ                           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at            TIMESTAMPTZ                           NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT provider_webhook_inbox_provider_account_id_event_id_key  UNIQUE (provider_account_id, provider_event_id),
    CONSTRAINT provider_webhook_inbox_provider_identity_fkey            FOREIGN KEY (store_id, provider_account_id, capability, provider) REFERENCES integration.provider_accounts (store_id, id, capability, provider) ON DELETE CASCADE,
    CONSTRAINT provider_webhook_inbox_provider_event_id_length_check    CHECK (length(trim(provider_event_id)) BETWEEN 1 AND 255),
    CONSTRAINT provider_webhook_inbox_provider_event_type_length_check  CHECK (length(trim(provider_event_type)) BETWEEN 1 AND 255),
    CONSTRAINT provider_webhook_inbox_normalized_event_type_check       CHECK (normalized_event_type IS NULL OR normalized_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    CONSTRAINT provider_webhook_inbox_payload_object_check              CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT provider_webhook_inbox_aggregate_shape_check             CHECK ((aggregate_type IS NULL AND aggregate_id IS NULL) OR (aggregate_type IS NOT NULL AND aggregate_type ~ '^[a-z][a-z0-9_]*$')),
    CONSTRAINT provider_webhook_inbox_status_timestamps_check           CHECK ((processing_status = 'pending' AND processed_at IS NULL AND unsupported_at IS NULL AND failed_at IS NULL) OR (processing_status = 'processed' AND processed_at IS NOT NULL AND unsupported_at IS NULL AND failed_at IS NULL) OR (processing_status = 'unsupported' AND processed_at IS NULL AND unsupported_at IS NOT NULL AND failed_at IS NULL) OR (processing_status = 'failed' AND processed_at IS NULL AND unsupported_at IS NULL AND failed_at IS NOT NULL)),
    CONSTRAINT provider_webhook_inbox_last_error_length_check           CHECK (last_error IS NULL OR length(last_error) <= 2000)
);

CREATE INDEX event_outbox_pending_idx ON integration.event_outbox (created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;
CREATE INDEX provider_accounts_store_capability_created_idx ON integration.provider_accounts (store_id, capability, created_at DESC, id DESC);
CREATE INDEX provider_webhook_inbox_claim_idx ON integration.provider_webhook_inbox (created_at, id) WHERE processing_status = 'pending';
CREATE INDEX provider_webhook_inbox_order_idx ON integration.provider_webhook_inbox (store_id, aggregate_id, created_at DESC) WHERE aggregate_id IS NOT NULL;

INSERT INTO integration.event_routes (internal_event_type, queue_name, description) VALUES ('search.product.changed', 'chaos_search_events', 'Refreshes the Store-isolated Product search document'), ('order.confirmed', 'chaos_email_commands', 'Sends the Order confirmation through the configured Email provider'), ('fulfillment.shipped', 'chaos_shipping_commands', 'Dispatches shipment state to the configured Shipping provider'), ('refund.create_requested', 'chaos_payment_commands', 'Creates an Order refund through the configured Payment provider');

CREATE FUNCTION integration.event_route_queue_name (requested_event_type TEXT)
RETURNS TEXT
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT registry.queue_name
    FROM integration.event_routes AS registry
    WHERE registry.internal_event_type = event_route_queue_name.requested_event_type;
$$;

CREATE FUNCTION integration.enqueue_event_outbox ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    resolved_queue_name TEXT;
    resolved_message_id BIGINT;
BEGIN
    resolved_queue_name := integration.event_route_queue_name(NEW.internal_event_type);
    IF resolved_queue_name IS NULL THEN
        RAISE EXCEPTION 'internal event type % has no queue route', NEW.internal_event_type
            USING ERRCODE = '23514';
    END IF;

    SELECT message_id
    INTO resolved_message_id
    FROM pgmq.send(
        resolved_queue_name,
        jsonb_build_object('version', 1, 'event_id', NEW.id)
    ) AS message_id;

    UPDATE integration.event_outbox AS event
    SET queue_name = resolved_queue_name,
        pgmq_message_id = resolved_message_id
    WHERE event.id = NEW.id;

    RETURN NULL;
END;
$$;

CREATE FUNCTION integration.claim_event_outbox (
    queue_name TEXT,
    batch_size INTEGER
)
RETURNS TABLE (
    id                  UUID,
    store_id            UUID,
    internal_event_type TEXT,
    aggregate_id        UUID,
    payload             JSONB,
    occurred_at         TIMESTAMPTZ,
    attempts            INTEGER
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    message RECORD;
    target  RECORD;
BEGIN
    IF queue_name NOT IN (
        'chaos_payment_commands',
        'chaos_email_commands',
        'chaos_shipping_commands',
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
        SELECT
            event.id,
            event.store_id,
            event.internal_event_type,
            event.aggregate_id,
            event.payload,
            event.created_at
        INTO target
        FROM integration.event_outbox AS event
        WHERE event.queue_name = queue_name
          AND event.pgmq_message_id = message.msg_id
          AND event.processed_at IS NULL
          AND event.failed_at IS NULL;

        IF NOT FOUND THEN
            PERFORM pgmq.delete(queue_name, message.msg_id);
            CONTINUE;
        END IF;

        id                  := target.id;
        store_id            := target.store_id;
        internal_event_type := target.internal_event_type;
        aggregate_id        := target.aggregate_id;
        payload             := target.payload;
        occurred_at         := target.created_at;
        attempts              := message.read_ct;

        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.finish_event_outbox (
    event_id      UUID,
    attempts      INTEGER,
    succeeded     BOOLEAN,
    failure       TEXT,
    max_attempts  INTEGER,
    finished_at   TIMESTAMPTZ
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
    SELECT event.pgmq_message_id, event.queue_name
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
            failed_at    = CASE WHEN succeeded THEN NULL ELSE finished_at END,
            last_error   = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
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

CREATE FUNCTION integration.resolve_provider_account (
    requested_capability  integration.provider_capability,
    requested_provider    TEXT,
    requested_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id            UUID,
    capability          integration.provider_capability,
    provider            TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.store_id, account.capability, account.provider
    FROM integration.provider_accounts AS account
    WHERE account.id = requested_account_id
      AND account.capability = requested_capability
      AND account.provider = requested_provider
      AND account.enabled;
$$;

CREATE FUNCTION integration.resolve_webhook_secret_reference (
    requested_capability  integration.provider_capability,
    requested_provider    TEXT,
    requested_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id            UUID,
    secret_reference    TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.store_id, account.webhook_secret_reference
    FROM integration.provider_accounts AS account
    WHERE account.id = requested_account_id
      AND account.capability = requested_capability
      AND account.provider = requested_provider
      AND account.enabled
      AND account.webhook_secret_reference IS NOT NULL;
$$;

CREATE FUNCTION integration.enqueue_webhook_event ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    resolved_message_id BIGINT;
BEGIN
    SELECT message_id
    INTO resolved_message_id
    FROM pgmq.send(
        'chaos_webhooks',
        jsonb_build_object(
            'version', 1,
            'webhook_event_id', NEW.id,
            'capability', NEW.capability::text
        )
    ) AS message_id;

    UPDATE integration.provider_webhook_inbox AS event
    SET pgmq_message_id = resolved_message_id
    WHERE event.id = NEW.id;

    RETURN NULL;
END;
$$;

CREATE FUNCTION integration.claim_provider_webhook_inbox (
    requested_capability integration.provider_capability,
    batch_size           INTEGER
)
RETURNS TABLE (
    id                    UUID,
    store_id              UUID,
    provider_account_id   UUID,
    capability            integration.provider_capability,
    provider              TEXT,
    provider_event_type   TEXT,
    normalized_event_type TEXT,
    payload               JSONB,
    attempts              INTEGER
)
LANGUAGE plpgsql
VOLATILE
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
            'chaos_webhooks',
            120,
            greatest(least(batch_size, 100), 1),
            jsonb_build_object('capability', requested_capability::text)
        ) AS queued
    LOOP
        SELECT event.id,
               event.store_id,
               event.provider_account_id,
               event.capability,
               event.provider,
               event.provider_event_type,
               event.normalized_event_type,
               event.payload
        INTO target
        FROM integration.provider_webhook_inbox AS event
        WHERE event.pgmq_message_id = message.msg_id
          AND event.capability = requested_capability
          AND event.processing_status = 'pending';

        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_webhooks', message.msg_id);
            CONTINUE;
        END IF;

        id                    := target.id;
        store_id              := target.store_id;
        provider_account_id   := target.provider_account_id;
        capability            := target.capability;
        provider              := target.provider;
        provider_event_type   := target.provider_event_type;
        normalized_event_type := target.normalized_event_type;
        payload               := target.payload;
        attempts            := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION integration.finish_provider_webhook (
    event_id          UUID,
    attempts          INTEGER,
    requested_outcome integration.webhook_processing_status,
    failure           TEXT,
    max_attempts      INTEGER,
    finished_at       TIMESTAMPTZ
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
    FROM integration.provider_webhook_inbox AS event
    WHERE event.id = event_id
      AND event.processing_status = 'pending'
    FOR UPDATE;

    IF message_id IS NULL THEN
        RETURN false;
    END IF;

    IF requested_outcome = 'processed' THEN
        UPDATE integration.provider_webhook_inbox AS event
        SET processing_status = 'processed',
            processed_at = finished_at,
            last_error = NULL
        WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSIF requested_outcome = 'unsupported' THEN
        UPDATE integration.provider_webhook_inbox AS event
        SET processing_status = 'unsupported',
            unsupported_at = finished_at,
            last_error = left(failure, 2000)
        WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSIF attempts >= greatest(max_attempts, 1) THEN
        UPDATE integration.provider_webhook_inbox AS event
        SET processing_status = 'failed',
            failed_at = finished_at,
            last_error = left(failure, 2000)
        WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSE
        UPDATE integration.provider_webhook_inbox AS event
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

CREATE FUNCTION integration.set_provider_webhook_aggregate (
    event_id           UUID,
    resolved_type      TEXT,
    resolved_aggregate UUID
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE integration.provider_webhook_inbox
    SET aggregate_type = resolved_type,
        aggregate_id = resolved_aggregate
    WHERE id = event_id
    RETURNING true;
$$;

CREATE FUNCTION commerce.capture_product_change ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    INSERT INTO integration.event_outbox (
        id,
        store_id,
        aggregate_type,
        aggregate_id,
        internal_event_type,
        payload
    ) VALUES (
        uuidv7(),
        NEW.store_id,
        'product',
        NEW.id,
        'search.product.changed',
        jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.capture_variant_change ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    owning_store_id    UUID;
    changed_product_id UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        owning_store_id    := OLD.store_id;
        changed_product_id := OLD.product_id;
    ELSE
        owning_store_id    := NEW.store_id;
        changed_product_id := NEW.product_id;
    END IF;

    IF EXISTS (SELECT 1 FROM commerce.stores WHERE id = owning_store_id) THEN
        INSERT INTO integration.event_outbox (
            id,
            store_id,
            aggregate_type,
            aggregate_id,
            internal_event_type,
            payload
        ) VALUES (
            uuidv7(),
            owning_store_id,
            'product',
            changed_product_id,
            'search.product.changed',
            jsonb_build_object('product_id', changed_product_id)
        );
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.rebuild_store_products (store_id UUID)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    product_id UUID;
    rebuilt    BIGINT := 0;
BEGIN
    DELETE FROM commerce.product_documents WHERE store_id = $1;

    FOR product_id IN
        SELECT id FROM commerce.products WHERE store_id = $1
    LOOP
        PERFORM commerce.refresh_product_document($1, product_id);
        rebuilt := rebuilt + 1;
    END LOOP;

    RETURN rebuilt;
END;
$$;

CREATE FUNCTION commerce.process_events (
    batch_size    INTEGER,
    max_attempts  INTEGER,
    finished_at   TIMESTAMPTZ
)
RETURNS BIGINT
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    event     RECORD;
    processed BIGINT := 0;
BEGIN
    FOR event IN
        SELECT
            outbox.id,
            outbox.store_id,
            outbox.aggregate_id,
            outbox.attempts
        FROM integration.claim_event_outbox(
            'chaos_search_events', batch_size
        ) AS outbox
    LOOP
        BEGIN
            PERFORM commerce.refresh_product_document(
                event.store_id, event.aggregate_id
            );
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, true, '', max_attempts, finished_at
            );
            processed := processed + 1;
        EXCEPTION WHEN OTHERS THEN
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, false, SQLERRM, max_attempts, finished_at
            );
        END;
    END LOOP;

    RETURN processed;
END;
$$;

CREATE TRIGGER event_outbox_enqueue AFTER INSERT ON integration.event_outbox FOR EACH ROW EXECUTE FUNCTION integration.enqueue_event_outbox();
CREATE TRIGGER provider_webhook_inbox_enqueue AFTER INSERT ON integration.provider_webhook_inbox FOR EACH ROW EXECUTE FUNCTION integration.enqueue_webhook_event();

ALTER TABLE integration.event_outbox ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.event_outbox
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE integration.provider_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE integration.provider_webhook_inbox ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.provider_webhook_inbox
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.event_route_queue_name (TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_event_outbox () FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_event_outbox (TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_event_outbox (UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.resolve_provider_account (integration.provider_capability, TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.resolve_webhook_secret_reference (integration.provider_capability, TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_webhook_event () FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_provider_webhook_inbox (integration.provider_capability, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_provider_webhook (UUID, INTEGER, integration.webhook_processing_status, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.set_provider_webhook_aggregate (UUID, TEXT, UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.rebuild_store_products (UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.process_events (INTEGER, INTEGER, TIMESTAMPTZ) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.finish_event_outbox (UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_event_outbox (TEXT, INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.resolve_provider_account (integration.provider_capability, TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.resolve_webhook_secret_reference (integration.provider_capability, TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_provider_webhook_inbox (integration.provider_capability, INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_provider_webhook (UUID, INTEGER, integration.webhook_processing_status, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.set_provider_webhook_aggregate (UUID, TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.rebuild_store_products (UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.process_events (INTEGER, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON integration.event_routes FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.event_outbox FROM chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
