SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_webhooks');

-- readiness holds the onboarding-check result ({status, snapshot,
-- checked_at}), checked synchronously when the account is created or
-- updated — there is no background re-check, so the shape stays small.
-- Changing a credential or webhook secret takes effect immediately — there
-- is no rotation grace window, so an operator-initiated change can fail
-- signature verification for webhook deliveries already in flight under the
-- old secret.
CREATE TABLE commerce.payment_provider_accounts (
    id                                 UUID        NOT NULL PRIMARY KEY,
    store_id                           UUID        NOT NULL,
    provider                           TEXT        NOT NULL,
    display_name                       TEXT        NOT NULL DEFAULT 'Payment provider',
    credential_secret_reference        TEXT,
    webhook_secret_reference           TEXT,
    readiness                          JSONB       NOT NULL DEFAULT '{"status": "unchecked"}',
    enabled                            BOOLEAN     NOT NULL DEFAULT false,
    created_at                         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT payment_provider_accounts_store_id_id_key                     UNIQUE (store_id, id),
    CONSTRAINT payment_provider_accounts_store_provider_key                  UNIQUE (store_id, provider),
    CONSTRAINT payment_provider_accounts_store_id_fkey                       FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT payment_provider_accounts_provider_length_check               CHECK (provider ~ '^[a-z0-9_]{1,64}$'),
    CONSTRAINT payment_provider_accounts_stripe_only_check                   CHECK (provider = 'stripe_checkout'),
    CONSTRAINT payment_provider_accounts_display_name_length_check           CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT payment_provider_accounts_credential_reference_check          CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_webhook_reference_check             CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_readiness_object_check              CHECK (jsonb_typeof(readiness) = 'object' AND pg_column_size(readiness) <= 8192)
);

CREATE INDEX payment_provider_accounts_store_created_idx ON commerce.payment_provider_accounts (store_id, created_at DESC, id DESC);

CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');

-- Each Refund is its own row against the Order it draws from, so a captured
-- Order can be partially refunded more than once with a full, queryable
-- history instead of one column being overwritten per call.
CREATE TABLE commerce.refunds (
    id                     UUID                     NOT NULL PRIMARY KEY,
    store_id               UUID                     NOT NULL,
    order_id               UUID                     NOT NULL,
    currency               CHAR(3)                  NOT NULL,
    status                 commerce.refund_status   NOT NULL DEFAULT 'pending',
    amount_minor           BIGINT                   NOT NULL,
    stripe_refund_id       TEXT,
    failure_code           TEXT,
    reason                 TEXT,
    provider_snapshot      JSONB,
    created_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT refunds_store_id_id_key                  UNIQUE (store_id, id),
    CONSTRAINT refunds_store_id_order_currency_fkey      FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders(store_id, id, currency),
    CONSTRAINT refunds_store_id_stripe_refund_key        UNIQUE (store_id, stripe_refund_id),
    CONSTRAINT refunds_amount_positive_check             CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check             CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_stripe_refund_check               CHECK (stripe_refund_id IS NULL OR stripe_refund_id ~ '^re_[A-Za-z0-9]+$'),
    CONSTRAINT refunds_failure_code_check                CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_failure_code_shape_check          CHECK (status = 'failed' OR failure_code IS NULL),
    CONSTRAINT refunds_reason_length_check               CHECK (reason IS NULL OR length(trim(reason)) BETWEEN 1 AND 2000),
    CONSTRAINT refunds_snapshot_size_check               CHECK (provider_snapshot IS NULL OR pg_column_size(provider_snapshot) <= 32768),
    CONSTRAINT refunds_snapshot_is_object_check          CHECK (provider_snapshot IS NULL OR jsonb_typeof(provider_snapshot) = 'object')
);

CREATE INDEX refunds_order_created_idx ON commerce.refunds (store_id, order_id, created_at DESC);

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
    requested_provider             TEXT,
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
      FROM commerce.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider             TEXT,
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
      FROM commerce.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled
       AND account.webhook_secret_reference IS NOT NULL;
$$;

-- order_id is filled in when a webhook is mapped to or resolved against its
-- Order (a payment or refund belongs to exactly one Order). It can remain
-- NULL for an ignored provider event or a webhook that fails before that
-- resolution. This lets support/debugging pull "every raw provider event for
-- this Order" without correlating payload contents against refunds by hand.
CREATE TABLE commerce.provider_webhooks (
    id                   UUID        NOT NULL PRIMARY KEY,
    store_id             UUID        NOT NULL,
    provider             TEXT        NOT NULL,
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
    CONSTRAINT provider_webhooks_store_id_provider_account_fkey               FOREIGN KEY (store_id, provider_account_id) REFERENCES commerce.payment_provider_accounts(store_id, id),
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
        provider := target.provider;
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

-- provider_webhooks is otherwise append-only for chaos_runtime (INSERT and
-- SELECT only; see the REVOKE below): a raw webhook snapshot must not be
-- editable by application code beyond what a handful of controlled
-- functions allow. This narrow function is the only path to backfill the
-- Order a webhook resolved to, once that Order is known.
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

ALTER TABLE commerce.payment_provider_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.provider_webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.payment_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

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

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_event_outbox(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.enqueue_webhook_event() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_webhook_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_event_outbox(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_webhook_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.payment_provider_accounts,
       commerce.provider_webhooks,
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
