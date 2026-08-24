CREATE SCHEMA integration;

SELECT pgmq.create('chaos_search_events');

CREATE TYPE integration.payment_provider AS ENUM ('stripe');
CREATE TYPE integration.shipping_provider AS ENUM ('manual');

CREATE TABLE integration.event_consumers (
    event_type  TEXT PRIMARY KEY,
    queue_name  TEXT NOT NULL,
    description TEXT NOT NULL,

    CONSTRAINT event_consumers_event_type_check        CHECK (event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    CONSTRAINT event_consumers_queue_name_check        CHECK (queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_consumers_description_check       CHECK (length(trim(description)) BETWEEN 1 AND 255)
);

CREATE TABLE integration.event_outbox (
    id              UUID        NOT NULL PRIMARY KEY,
    store_id        UUID        NOT NULL,
    aggregate_type  TEXT        NOT NULL,
    aggregate_id    UUID        NOT NULL,
    event_type      TEXT        NOT NULL,
    payload         JSONB       NOT NULL,
    queue_name      TEXT        NOT NULL,
    pgmq_message_id BIGINT      NOT NULL,
    processed_at    TIMESTAMPTZ,
    failed_at       TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT event_outbox_queue_name_pgmq_message_id_key   UNIQUE (queue_name, pgmq_message_id),
    CONSTRAINT event_outbox_store_id_fkey                    FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT event_outbox_event_type_fkey                  FOREIGN KEY (event_type) REFERENCES integration.event_consumers (event_type),
    CONSTRAINT event_outbox_payload_object_check             CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT event_outbox_queue_name_check                 CHECK (queue_name ~ '^chaos_[a-z][a-z0-9_]*$'),
    CONSTRAINT event_outbox_completion_check                 CHECK (processed_at IS NULL OR failed_at IS NULL)
);

CREATE TABLE integration.payment_provider_accounts (
    id                           UUID                          NOT NULL PRIMARY KEY,
    store_id                     UUID                          NOT NULL,
    provider                     integration.payment_provider  NOT NULL,
    display_name                 TEXT                          NOT NULL DEFAULT 'Payment Provider',
    credential_secret_reference  TEXT,
    webhook_secret_reference     TEXT,
    meta                         JSONB,
    created_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT payment_provider_accounts_store_provider_key          UNIQUE (store_id, provider),
    CONSTRAINT payment_provider_accounts_store_id_fkey               FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT payment_provider_accounts_display_name_length_check   CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT payment_provider_accounts_credential_reference_check  CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_webhook_reference_check     CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_meta_object_check           CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT payment_provider_accounts_meta_size_check             CHECK (meta IS NULL OR pg_column_size(meta) <= 32768)
);

CREATE TABLE integration.shipping_provider_accounts (
    id                           UUID                          NOT NULL PRIMARY KEY,
    store_id                     UUID                          NOT NULL,
    provider                     integration.shipping_provider NOT NULL,
    display_name                 TEXT                          NOT NULL DEFAULT 'Shipping Provider',
    credential_secret_reference  TEXT,
    webhook_secret_reference     TEXT,
    meta                         JSONB,
    created_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT shipping_provider_accounts_store_provider_key          UNIQUE (store_id, provider),
    CONSTRAINT shipping_provider_accounts_store_id_fkey               FOREIGN KEY (store_id) REFERENCES commerce.stores (id) ON DELETE CASCADE,
    CONSTRAINT shipping_provider_accounts_display_name_length_check   CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT shipping_provider_accounts_credential_reference_check  CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT shipping_provider_accounts_webhook_reference_check     CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT shipping_provider_accounts_meta_object_check           CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT shipping_provider_accounts_meta_size_check             CHECK (meta IS NULL OR pg_column_size(meta) <= 32768)
);

CREATE INDEX event_outbox_pending_idx
    ON integration.event_outbox (created_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;

CREATE INDEX payment_provider_accounts_store_created_idx
    ON integration.payment_provider_accounts (store_id, created_at DESC, id DESC);

CREATE INDEX shipping_provider_accounts_store_created_idx
    ON integration.shipping_provider_accounts (store_id, created_at DESC, id DESC);

CREATE FUNCTION integration.event_queue_name (event_type TEXT)
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

CREATE FUNCTION integration.enqueue_event_outbox ()
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

CREATE FUNCTION integration.claim_routed_event_outbox (
    queue_name TEXT,
    batch_size INTEGER
)
RETURNS TABLE (
    id           UUID,
    store_id     UUID,
    event_type   TEXT,
    aggregate_id UUID,
    payload      JSONB,
    occurred_at  TIMESTAMPTZ,
    attempts     INTEGER
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
    IF queue_name NOT IN ('chaos_payment_commands', 'chaos_search_events') THEN
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
            event.event_type,
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

        id           := target.id;
        store_id     := target.store_id;
        event_type   := target.event_type;
        aggregate_id := target.aggregate_id;
        payload      := target.payload;
        occurred_at  := target.created_at;
        attempts     := message.read_ct;

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
        event_type,
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
            event_type,
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
        FROM integration.claim_routed_event_outbox(
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

ALTER TABLE integration.event_outbox ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.event_outbox
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE integration.payment_provider_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.payment_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE integration.shipping_provider_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.shipping_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

INSERT INTO integration.event_consumers (event_type, queue_name, description)
VALUES (
    'search.product.changed',
    'chaos_search_events',
    'Refreshes the Store-isolated Product search document'
);

CREATE TRIGGER event_outbox_enqueue
    BEFORE INSERT ON integration.event_outbox
    FOR EACH ROW
    EXECUTE FUNCTION integration.enqueue_event_outbox();

CREATE TRIGGER products_search_change
    AFTER INSERT OR UPDATE OF handle, title, description ON commerce.products
    FOR EACH ROW
    EXECUTE FUNCTION commerce.capture_product_change();

CREATE TRIGGER variants_search_change
    AFTER INSERT OR UPDATE OF title, sku OR DELETE ON commerce.product_variants
    FOR EACH ROW
    EXECUTE FUNCTION commerce.capture_variant_change();

REVOKE ALL ON FUNCTION integration.event_queue_name (TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.enqueue_event_outbox () FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_routed_event_outbox (TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_event_outbox (UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.rebuild_store_products (UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.process_events (INTEGER, INTEGER, TIMESTAMPTZ) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.finish_event_outbox (UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.rebuild_store_products (UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.process_events (INTEGER, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON integration.event_consumers FROM chaos_runtime;
REVOKE UPDATE, DELETE ON integration.event_outbox FROM chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
