SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_webhooks');

CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');

CREATE TABLE commerce.refunds (
    id                     UUID                     NOT NULL PRIMARY KEY,
    store_id               UUID                     NOT NULL,
    order_id               UUID                     NOT NULL,
    currency               CHAR(3)                  NOT NULL,
    status                 commerce.refund_status   NOT NULL DEFAULT 'pending',
    amount_minor           BIGINT                   NOT NULL,
    payment_provider_account_id UUID                     NOT NULL,
    payment_provider_reference_id TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT refunds_store_id_id_key                  UNIQUE (store_id, id),
    CONSTRAINT refunds_store_id_order_currency_fkey      FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders(store_id, id, currency),
    CONSTRAINT refunds_payment_provider_account_fkey     FOREIGN KEY (payment_provider_account_id) REFERENCES integration.payment_provider_accounts(id),
    CONSTRAINT refunds_amount_positive_check             CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_payment_provider_reference_check CHECK (payment_provider_reference_id IS NULL OR length(trim(payment_provider_reference_id)) BETWEEN 1 AND 255),
    CONSTRAINT refunds_failure_code_check                CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_failure_code_shape_check          CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE INDEX refunds_order_created_idx ON commerce.refunds (store_id, order_id, created_at DESC);
CREATE UNIQUE INDEX refunds_payment_provider_reference_key
    ON commerce.refunds (store_id, payment_provider_account_id, payment_provider_reference_id)
    WHERE payment_provider_reference_id IS NOT NULL;

CREATE FUNCTION commerce.claim_event_outbox(
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

CREATE FUNCTION commerce.resolve_provider_account(
    requested_provider             integration.payment_provider,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id            UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.store_id
     FROM integration.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider             integration.payment_provider,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    secret_reference    TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.webhook_secret_reference
      FROM integration.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.webhook_secret_reference IS NOT NULL;
$$;

CREATE TABLE commerce.provider_webhooks (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    provider_account_id  UUID        NOT NULL,
    provider_event_id    TEXT        NOT NULL,
    event_type           TEXT        NOT NULL,
    payload              JSONB       NOT NULL,
    order_id             UUID,
    pgmq_message_id      BIGINT      NOT NULL UNIQUE,
    processed_at         TIMESTAMPTZ,
    failed_at            TIMESTAMPTZ,
    last_error           TEXT,
    verified_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT provider_webhooks_provider_account_id_provider_event_id_key    UNIQUE (provider_account_id, provider_event_id),
    CONSTRAINT provider_webhooks_store_id_fkey                                FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT provider_webhooks_provider_account_fkey                         FOREIGN KEY (provider_account_id) REFERENCES integration.payment_provider_accounts(id),
    CONSTRAINT provider_webhooks_store_id_order_fkey                          FOREIGN KEY (store_id, order_id) REFERENCES commerce.orders(store_id, id),
    CONSTRAINT provider_webhooks_payload_object_check                         CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT provider_webhooks_completion_check                             CHECK (processed_at IS NULL OR failed_at IS NULL)
);

CREATE INDEX provider_webhooks_claim_idx ON commerce.provider_webhooks (created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;
CREATE INDEX provider_webhooks_order_idx ON commerce.provider_webhooks (store_id, order_id, created_at DESC) WHERE order_id IS NOT NULL;
CREATE FUNCTION commerce.enqueue_webhook_event()
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

CREATE FUNCTION commerce.claim_webhook_events(
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
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
               event.event_type,
               event.payload
          INTO target
          FROM commerce.provider_webhooks AS event
         WHERE event.pgmq_message_id = message.msg_id
           AND event.processed_at IS NULL
           AND event.failed_at IS NULL;
        IF NOT FOUND THEN
            PERFORM pgmq.delete('chaos_webhooks', message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        event_type := target.event_type;
        payload := target.payload;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;

CREATE FUNCTION commerce.finish_webhook_event(
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
      FROM commerce.provider_webhooks AS event
     WHERE event.id = event_id
       AND event.processed_at IS NULL
       AND event.failed_at IS NULL
     FOR UPDATE;
    IF message_id IS NULL THEN
        RETURN false;
    END IF;
    IF succeeded OR attempts >= greatest(max_attempts, 1) THEN
        UPDATE commerce.provider_webhooks AS event
           SET processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
               failed_at = CASE WHEN succeeded THEN NULL ELSE finished_at END,
               last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
         WHERE event.id = event_id;
        PERFORM pgmq.delete('chaos_webhooks', message_id);
    ELSE
        UPDATE commerce.provider_webhooks AS event
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

CREATE FUNCTION commerce.set_webhook_order_id(
    event_id UUID,
    resolved_order_id UUID
)
RETURNS BOOLEAN
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    UPDATE commerce.provider_webhooks
       SET order_id = resolved_order_id
     WHERE id = event_id;
    RETURN FOUND;
END;
$$;

CREATE INDEX provider_webhooks_provider_account_idx ON commerce.provider_webhooks (provider_account_id, created_at, id) WHERE processed_at IS NULL AND failed_at IS NULL;

ALTER TABLE commerce.provider_webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.provider_webhooks
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.refunds
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE TRIGGER provider_webhooks_enqueue
    BEFORE INSERT ON commerce.provider_webhooks
    FOR EACH ROW
    EXECUTE FUNCTION commerce.enqueue_webhook_event();

INSERT INTO integration.event_consumers (event_type, queue_name, description)
VALUES
    ('refund.create_requested', 'chaos_payment_commands', 'Creates a Stripe Refund for the Order');

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(integration.payment_provider, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_event_outbox(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(integration.payment_provider, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.enqueue_webhook_event() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_webhook_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(integration.payment_provider, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_event_outbox(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_provider_webhook_secret_references(integration.payment_provider, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_webhook_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.provider_webhooks,
       commerce.refunds
    TO chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.provider_webhooks FROM chaos_runtime;
REVOKE DELETE ON commerce.refunds FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
