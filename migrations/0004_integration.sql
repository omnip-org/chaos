CREATE SCHEMA integration;

-- Topic-routed queues (pgmq.bind_topic/pgmq.send_topic, native since pgmq
-- v1.11.0): each consumer below is fully isolated on its own queue, fed by
-- one publish_commerce_event call fanning a routing key out to every bound
-- queue atomically. There is no delivery-row table backing any of this —
-- PGMQ's own message lifecycle (visibility timeout retry, archive() on
-- exhausted retries, see finish_topic_event below) is the only durability
-- relied on; a failure is logged by the consuming worker, not persisted.
SELECT pgmq.create('search_index_queue');
SELECT pgmq.create('analytics_capi_queue');
SELECT pgmq.create('notification_email_queue');
SELECT pgmq.create('payment_commands_queue');
SELECT pgmq.create('shipping_commands_queue');
SELECT pgmq.create('payment_webhooks_queue');
SELECT pgmq.create('email_webhooks_queue');

SELECT pgmq.bind_topic('product.updated',   'search_index_queue');
SELECT pgmq.bind_topic('payment.initiated', 'analytics_capi_queue');
SELECT pgmq.bind_topic('payment.completed', 'analytics_capi_queue');
SELECT pgmq.bind_topic('payment.completed', 'notification_email_queue');
-- A manual admin order confirmation (PostgresOrderManagementRepository::
-- transition_order) has no captured payment or analytics event behind it,
-- so it only ever notifies email, never CAPI.
SELECT pgmq.bind_topic('order.confirmed',   'notification_email_queue');
SELECT pgmq.bind_topic('refund.create_requested', 'payment_commands_queue');
SELECT pgmq.bind_topic('fulfillment.shipped',     'shipping_commands_queue');
-- Verified provider webhooks route by capability, one queue per consumer —
-- shipping has no webhook consumer today (manual shipping only), so there
-- is no third binding here.
SELECT pgmq.bind_topic('webhook.payment', 'payment_webhooks_queue');
SELECT pgmq.bind_topic('webhook.email',   'email_webhooks_queue');

CREATE TYPE integration.provider_capability AS ENUM ('email', 'payment', 'shipping', 'analytics');

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

CREATE INDEX provider_accounts_store_capability_created_idx ON integration.provider_accounts (store_id, capability, created_at DESC, id DESC);

CREATE FUNCTION integration.prevent_provider_account_identity_change ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.store_id IS DISTINCT FROM OLD.store_id
       OR NEW.capability IS DISTINCT FROM OLD.capability
       OR NEW.provider IS DISTINCT FROM OLD.provider THEN
        RAISE EXCEPTION 'Provider Account identity is immutable after creation'
            USING ERRCODE = '22023';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_accounts_identity_immutable
    BEFORE UPDATE OF id, store_id, capability, provider
    ON integration.provider_accounts
    FOR EACH ROW EXECUTE FUNCTION integration.prevent_provider_account_identity_change();

-- One narrow entry point into the pgmq schema for producers: every business
-- transaction that needs to notify a consumer calls this from inside its own
-- transaction, so a rolled-back transaction never delivers a message a
-- consumer would act on.
CREATE FUNCTION integration.publish_commerce_event (
    routing_key TEXT,
    payload     JSONB
)
RETURNS INTEGER
LANGUAGE SQL
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT pgmq.send_topic(routing_key, payload);
$$;

-- One generic claim/finish pair, parametrized by queue name, reused by
-- every topic-routed consumer in this schema.
CREATE FUNCTION integration.claim_topic_queue (
    requested_queue_name TEXT,
    batch_size            INTEGER
)
RETURNS TABLE (
    msg_id    BIGINT,
    payload   JSONB,
    attempts  INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT queued.msg_id, queued.message, queued.read_ct
    FROM pgmq.read(
        requested_queue_name,
        120,
        greatest(least(batch_size, 100), 1),
        '{}'::jsonb
    ) AS queued;
$$;

CREATE FUNCTION integration.finish_topic_event (
    requested_queue_name TEXT,
    requested_msg_id     BIGINT,
    attempts             INTEGER,
    succeeded            BOOLEAN,
    max_attempts         INTEGER
)
RETURNS VOID
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF succeeded THEN
        PERFORM pgmq.delete(requested_queue_name, requested_msg_id);
    ELSIF attempts >= greatest(max_attempts, 1) THEN
        PERFORM pgmq.archive(requested_queue_name, requested_msg_id);
    ELSE
        PERFORM pgmq.set_vt(
            requested_queue_name,
            requested_msg_id,
            least(power(2, greatest(attempts - 1, 0))::integer, 300)
        );
    END IF;
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

ALTER TABLE integration.provider_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

REVOKE ALL ON FUNCTION integration.publish_commerce_event (TEXT, JSONB) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_topic_queue (TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_topic_event (TEXT, BIGINT, INTEGER, BOOLEAN, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.resolve_provider_account (integration.provider_capability, TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.resolve_webhook_secret_reference (integration.provider_capability, TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.prevent_provider_account_identity_change () FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.publish_commerce_event (TEXT, JSONB) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_topic_queue (TEXT, INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_topic_event (TEXT, BIGINT, INTEGER, BOOLEAN, INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.resolve_provider_account (integration.provider_capability, TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.resolve_webhook_secret_reference (integration.provider_capability, TEXT, UUID) TO chaos_runtime;

GRANT USAGE ON SCHEMA integration TO chaos_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE UPDATE ON integration.provider_accounts FROM chaos_runtime;
GRANT UPDATE (
    display_name,
    credential_secret_reference,
    webhook_secret_reference,
    configuration,
    enabled,
    updated_at
)
    ON integration.provider_accounts TO chaos_runtime;
REVOKE DELETE, TRUNCATE ON integration.provider_accounts FROM chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT SELECT, INSERT ON TABLES TO chaos_runtime;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
