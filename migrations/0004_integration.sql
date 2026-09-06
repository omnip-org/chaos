CREATE SCHEMA integration;

SELECT pgmq.create('search_index_queue');
SELECT pgmq.create('analytics_capi_queue');
SELECT pgmq.create('notification_email_queue');
SELECT pgmq.create('provider_webhooks_queue');

SELECT pgmq.bind_topic('product.updated',                  'search_index_queue');
SELECT pgmq.bind_topic('order.payment.initiated',          'analytics_capi_queue');
SELECT pgmq.bind_topic('order.payment.completed',          'analytics_capi_queue');
SELECT pgmq.bind_topic('order.payment.completed',          'notification_email_queue');
SELECT pgmq.bind_topic('order.payment.partially_refunded', 'notification_email_queue');
SELECT pgmq.bind_topic('order.payment.refunded',           'notification_email_queue');
SELECT pgmq.bind_topic('order.fulfillment.shipped',        'notification_email_queue');
SELECT pgmq.bind_topic('order.fulfillment.delivered',      'notification_email_queue');
SELECT pgmq.bind_topic('provider.webhook.received',        'provider_webhooks_queue');

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

-- Every verified provider webhook lands here first: one append-only row per
-- (provider account, provider event id). The insert is the dedup — a provider
-- retry hits the unique constraint and no second job is enqueued. The same
-- transaction publishes `provider.webhook.received` onto
-- `provider_webhooks_queue`; a worker drains that queue, applies the event
-- through the owning capability, and stamps `processed_at`. `processed_at IS
-- NULL` past a grace period is the "stuck webhook" signal.
CREATE TABLE integration.provider_webhook_audit (
    id                     UUID                            NOT NULL PRIMARY KEY,
    store_id               UUID                            NOT NULL,
    provider_account_id    UUID                            NOT NULL,
    capability             integration.provider_capability NOT NULL,
    provider               TEXT                            NOT NULL,
    provider_event_id      TEXT                            NOT NULL,
    provider_event_type    TEXT                            NOT NULL,
    normalized_event_type  TEXT,
    payload                JSONB                           NOT NULL,
    received_at            TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,
    processed_at           TIMESTAMPTZ,

    CONSTRAINT provider_webhook_audit_dedup_key              UNIQUE (provider_account_id, provider_event_id),
    CONSTRAINT provider_webhook_audit_account_fkey           FOREIGN KEY (store_id, provider_account_id) REFERENCES integration.provider_accounts (store_id, id) ON DELETE CASCADE,
    CONSTRAINT provider_webhook_audit_provider_format_check  CHECK (provider ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT provider_webhook_audit_payload_object_check   CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT provider_webhook_audit_payload_size_check     CHECK (pg_column_size(payload) <= 524288)
);

CREATE INDEX provider_webhook_audit_store_received_idx ON integration.provider_webhook_audit (store_id, received_at DESC, id DESC);
CREATE INDEX provider_webhook_audit_unprocessed_idx ON integration.provider_webhook_audit (received_at) WHERE processed_at IS NULL;

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

ALTER TABLE integration.provider_webhook_audit ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.provider_webhook_audit
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

REVOKE UPDATE, DELETE, TRUNCATE ON integration.provider_webhook_audit FROM chaos_runtime;
GRANT UPDATE (processed_at) ON integration.provider_webhook_audit TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration GRANT SELECT, INSERT ON TABLES TO chaos_runtime;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
