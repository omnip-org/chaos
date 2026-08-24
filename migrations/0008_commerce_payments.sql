SELECT pgmq.create('chaos_payment_commands');
SELECT pgmq.create('chaos_webhooks');

CREATE TABLE commerce.payment_provider_accounts (
    id                                 UUID        NOT NULL PRIMARY KEY,
    store_id                           UUID        NOT NULL,
    provider                           TEXT        NOT NULL,
    display_name                       TEXT        NOT NULL DEFAULT 'Payment provider',
    credential_secret_reference        TEXT,
    previous_credential_secret_reference TEXT,
    credential_rotation_expires_at     TIMESTAMPTZ,
    webhook_secret_reference           TEXT,
    previous_webhook_secret_reference  TEXT,
    webhook_rotation_expires_at        TIMESTAMPTZ,
    readiness_status                   TEXT        NOT NULL DEFAULT 'unchecked',
    readiness_snapshot                 JSONB,
    readiness_checked_at               TIMESTAMPTZ,
    readiness_valid_until              TIMESTAMPTZ,
    readiness_reconcile_at             TIMESTAMPTZ,
    readiness_locked_by                UUID,
    readiness_locked_at                TIMESTAMPTZ,
    readiness_reconcile_attempts       INTEGER     NOT NULL DEFAULT 0,
    readiness_last_error               TEXT,
    enabled                            BOOLEAN     NOT NULL DEFAULT false,
    created_by_user_id                 UUID,
    created_at                         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT payment_provider_accounts_store_id_id_key                     UNIQUE (store_id, id),
    CONSTRAINT payment_provider_accounts_store_provider_key                  UNIQUE (store_id, provider),
    CONSTRAINT payment_provider_accounts_store_id_fkey                       FOREIGN KEY (store_id) REFERENCES commerce.stores(id),
    CONSTRAINT payment_provider_accounts_created_by_fkey                     FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT payment_provider_accounts_provider_length_check               CHECK (provider ~ '^[a-z0-9_]{1,64}$'),
    CONSTRAINT payment_provider_accounts_stripe_only_check                   CHECK (provider = 'stripe_checkout'),
    CONSTRAINT payment_provider_accounts_display_name_length_check           CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT payment_provider_accounts_credential_reference_check          CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_previous_credential_reference_check CHECK (previous_credential_secret_reference IS NULL OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(previous_credential_secret_reference) <= 32768 AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_credential_rotation_shape_check     CHECK ((previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL) OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)),
    CONSTRAINT payment_provider_accounts_webhook_reference_check             CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_previous_webhook_reference_check    CHECK (previous_webhook_secret_reference IS NULL OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(previous_webhook_secret_reference) <= 32768 AND previous_webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_webhook_rotation_shape_check        CHECK ((previous_webhook_secret_reference IS NULL AND webhook_rotation_expires_at IS NULL) OR (previous_webhook_secret_reference IS NOT NULL AND webhook_rotation_expires_at IS NOT NULL)),
    CONSTRAINT payment_provider_accounts_readiness_status_check              CHECK (readiness_status IN ('unchecked', 'ready', 'action_required')),
    CONSTRAINT payment_provider_accounts_readiness_shape_check               CHECK ((readiness_status = 'unchecked' AND readiness_snapshot IS NULL AND readiness_checked_at IS NULL AND readiness_valid_until IS NULL AND readiness_reconcile_at IS NULL) OR (readiness_status <> 'unchecked' AND jsonb_typeof(readiness_snapshot) = 'object' AND pg_column_size(readiness_snapshot) <= 8192 AND readiness_checked_at IS NOT NULL)),
    CONSTRAINT payment_provider_accounts_enabled_readiness_check             CHECK (NOT enabled OR (readiness_status = 'ready' AND readiness_valid_until IS NOT NULL AND readiness_reconcile_at IS NOT NULL)),
    CONSTRAINT payment_provider_accounts_readiness_validity_check            CHECK ((readiness_status = 'ready' AND readiness_valid_until > readiness_checked_at AND readiness_reconcile_at IS NOT NULL) OR (readiness_status <> 'ready' AND readiness_valid_until IS NULL AND readiness_reconcile_at IS NULL)),
    CONSTRAINT payment_provider_accounts_readiness_lock_shape_check          CHECK ((readiness_locked_by IS NULL AND readiness_locked_at IS NULL) OR (readiness_locked_by IS NOT NULL AND readiness_locked_at IS NOT NULL)),
    CONSTRAINT payment_provider_accounts_readiness_attempts_check            CHECK (readiness_reconcile_attempts BETWEEN 0 AND 31),
    CONSTRAINT payment_provider_accounts_readiness_error_length_check        CHECK (readiness_last_error IS NULL OR length(readiness_last_error) BETWEEN 1 AND 2000)
);

CREATE INDEX payment_provider_accounts_store_created_idx ON commerce.payment_provider_accounts (store_id, created_at DESC, id DESC);
CREATE INDEX payment_provider_accounts_readiness_due_idx ON commerce.payment_provider_accounts (readiness_reconcile_at, id) WHERE enabled;

CREATE TYPE commerce.payment_attempt_status AS ENUM ('pending', 'authorized', 'captured', 'failed', 'cancelled');
CREATE TYPE commerce.refund_status AS ENUM ('pending', 'succeeded', 'failed');

-- Each real attempt to pay an Order is its own row with its own identity, so
-- a decline followed by a retry creates a second attempt with a fresh Stripe
-- Checkout Session instead of colliding with the first attempt's reference.
CREATE TABLE commerce.payment_attempts (
    id                           UUID                              NOT NULL PRIMARY KEY,
    store_id                     UUID                              NOT NULL,
    order_id                     UUID                              NOT NULL,
    currency                     CHAR(3)                           NOT NULL,
    status                       commerce.payment_attempt_status   NOT NULL DEFAULT 'pending',
    amount_minor                 BIGINT                            NOT NULL,
    stripe_checkout_session_id   TEXT,
    stripe_payment_intent_id     TEXT,
    stripe_charge_id             TEXT,
    failure_code                 TEXT,
    provider_snapshot            JSONB,
    created_at                   TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT payment_attempts_store_id_id_key               UNIQUE (store_id, id),
    CONSTRAINT payment_attempts_store_id_order_currency_fkey  FOREIGN KEY (store_id, order_id, currency) REFERENCES commerce.orders(store_id, id, currency),
    CONSTRAINT payment_attempts_store_id_checkout_session_key UNIQUE (store_id, stripe_checkout_session_id),
    CONSTRAINT payment_attempts_store_id_payment_intent_key   UNIQUE (store_id, stripe_payment_intent_id),
    CONSTRAINT payment_attempts_amount_positive_check         CHECK (amount_minor > 0),
    CONSTRAINT payment_attempts_currency_format_check         CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT payment_attempts_checkout_session_check        CHECK (stripe_checkout_session_id IS NULL OR length(trim(stripe_checkout_session_id)) BETWEEN 1 AND 255),
    CONSTRAINT payment_attempts_payment_intent_check          CHECK (stripe_payment_intent_id IS NULL OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'),
    CONSTRAINT payment_attempts_charge_check                  CHECK (stripe_charge_id IS NULL OR stripe_charge_id ~ '^ch_[A-Za-z0-9]+$'),
    CONSTRAINT payment_attempts_failure_code_check            CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 2000),
    CONSTRAINT payment_attempts_failure_code_shape_check      CHECK (status = 'failed' OR failure_code IS NULL),
    CONSTRAINT payment_attempts_snapshot_size_check           CHECK (provider_snapshot IS NULL OR pg_column_size(provider_snapshot) <= 32768),
    CONSTRAINT payment_attempts_snapshot_is_object_check      CHECK (provider_snapshot IS NULL OR jsonb_typeof(provider_snapshot) = 'object')
);

CREATE INDEX payment_attempts_order_created_idx ON commerce.payment_attempts (store_id, order_id, created_at DESC);

-- Each Refund is its own row against the Payment Attempt it draws from, so a
-- captured Payment Attempt can be partially refunded more than once with a
-- full, queryable history instead of one column being overwritten per call.
CREATE TABLE commerce.refunds (
    id                     UUID                     NOT NULL PRIMARY KEY,
    store_id               UUID                     NOT NULL,
    payment_attempt_id     UUID                     NOT NULL,
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
    CONSTRAINT refunds_store_id_payment_attempt_fkey     FOREIGN KEY (store_id, payment_attempt_id) REFERENCES commerce.payment_attempts(store_id, id),
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
CREATE INDEX refunds_payment_attempt_idx ON commerce.refunds (store_id, payment_attempt_id, created_at DESC);

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
    SELECT account.id, candidate.secret_reference
      FROM commerce.payment_provider_accounts AS account
      CROSS JOIN LATERAL (
          VALUES
              (account.webhook_secret_reference, 0),
              (
                  CASE WHEN account.webhook_rotation_expires_at > CURRENT_TIMESTAMP
                       THEN account.previous_webhook_secret_reference END,
                  1
              )
      ) AS candidate(secret_reference, priority)
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled
       AND candidate.secret_reference IS NOT NULL
     ORDER BY candidate.priority;
$$;

CREATE FUNCTION commerce.resolve_store_provider_webhook_secret_references(
    requested_provider TEXT,
    requested_store_id UUID
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
    SELECT account.id, candidate.secret_reference
      FROM commerce.payment_provider_accounts AS account
      CROSS JOIN LATERAL (
          VALUES
              (account.webhook_secret_reference, 0),
              (
                  CASE WHEN account.webhook_rotation_expires_at > CURRENT_TIMESTAMP
                       THEN account.previous_webhook_secret_reference END,
                  1
              )
      ) AS candidate(secret_reference, priority)
     WHERE account.provider = requested_provider
       AND account.store_id = requested_store_id
       AND account.enabled
       AND candidate.secret_reference IS NOT NULL
     ORDER BY account.id, candidate.priority;
$$;

CREATE FUNCTION commerce.claim_provider_readiness_checks(
    worker_id   UUID,
    batch_size  INTEGER,
    claimed_at  TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id       UUID,
    store_id                  UUID,
    provider                  TEXT,
    credential_secret_reference TEXT,
    attempts                  INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE commerce.payment_provider_accounts AS account
           SET enabled = false,
               readiness_status = 'action_required',
               readiness_snapshot = jsonb_set(
                   jsonb_set(account.readiness_snapshot, '{ready}', 'false'::jsonb, true),
                   '{blocker_codes}', '["readiness_expired"]'::jsonb, true
               ),
               readiness_valid_until = NULL,
               readiness_reconcile_at = NULL,
               readiness_locked_by = NULL,
               readiness_locked_at = NULL,
               readiness_last_error = NULL,
               updated_at = claimed_at
         WHERE account.enabled
           AND account.readiness_valid_until <= claimed_at
        RETURNING account.id
    ), claimable AS (
        SELECT account.id
          FROM commerce.payment_provider_accounts AS account
         WHERE account.enabled
           AND account.credential_secret_reference IS NOT NULL
           AND account.readiness_valid_until > claimed_at
           AND account.readiness_reconcile_at <= claimed_at
           AND (
               account.readiness_locked_at IS NULL
               OR account.readiness_locked_at <= stale_before
           )
         ORDER BY account.readiness_reconcile_at, account.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE commerce.payment_provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

CREATE FUNCTION commerce.finish_provider_readiness_check(
    requested_provider_account_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    ready BOOLEAN,
    requested_readiness_snapshot JSONB,
    observed_at TIMESTAMPTZ,
    failure_message TEXT
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE commerce.payment_provider_accounts AS account
       SET enabled = CASE
               WHEN succeeded THEN account.enabled AND ready
               ELSE account.enabled
           END,
           readiness_status = CASE
               WHEN succeeded AND ready THEN 'ready'
               WHEN succeeded THEN 'action_required'
               ELSE account.readiness_status
           END,
           readiness_snapshot = CASE
               WHEN succeeded THEN requested_readiness_snapshot
               ELSE account.readiness_snapshot
           END,
           readiness_checked_at = CASE
               WHEN succeeded THEN observed_at
               ELSE account.readiness_checked_at
           END,
           readiness_valid_until = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '24 hours'
               WHEN succeeded THEN NULL
               ELSE account.readiness_valid_until
           END,
           readiness_reconcile_at = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '6 hours'
               WHEN succeeded THEN NULL
               ELSE observed_at + make_interval(
                   secs => least(power(2, greatest(account.readiness_reconcile_attempts - 1, 0))::integer, 3600)
               )
           END,
           readiness_locked_by = NULL,
           readiness_locked_at = NULL,
           readiness_reconcile_attempts = CASE
               WHEN succeeded THEN 0
               ELSE account.readiness_reconcile_attempts
           END,
           readiness_last_error = CASE
               WHEN succeeded THEN NULL
               ELSE COALESCE(NULLIF(left(failure_message, 2000), ''), 'readiness check failed')
           END,
           updated_at = CASE WHEN succeeded THEN observed_at ELSE account.updated_at END
     WHERE account.id = requested_provider_account_id
       AND account.readiness_locked_by = worker_id
    RETURNING true;
$$;

-- order_id is filled in once the webhook is processed and its Order is
-- known (a Payment Attempt or Refund always belongs to exactly one Order);
-- it stays NULL for a webhook that fails before that resolution. This lets
-- support/debugging pull "every raw provider event for this Order" without
-- correlating payload contents against payment_attempts/refunds by hand.
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
ALTER TABLE commerce.payment_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.refunds ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.payment_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.provider_webhooks
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.payment_attempts
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
REVOKE ALL ON FUNCTION commerce.resolve_store_provider_webhook_secret_references(TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_provider_readiness_checks(UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.finish_provider_readiness_check(UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.enqueue_webhook_event() FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_webhook_events(INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_event_outbox(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_store_provider_webhook_secret_references(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_provider_readiness_checks(UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.finish_provider_readiness_check(UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_webhook_events(INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.finish_webhook_event(UUID, INTEGER, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.set_webhook_order_id(UUID, UUID) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.payment_provider_accounts,
       commerce.provider_webhooks,
       commerce.payment_attempts,
       commerce.refunds
    TO chaos_runtime;

REVOKE UPDATE, DELETE ON commerce.provider_webhooks FROM chaos_runtime;
REVOKE DELETE ON commerce.payment_attempts, commerce.refunds FROM chaos_runtime;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
