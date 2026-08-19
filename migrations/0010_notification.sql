CREATE TYPE notification.email_delivery_status AS ENUM (
    'pending',
    'processing',
    'sent',
    'delivered',
    'bounced',
    'complained',
    'suppressed',
    'failed',
    'dead_letter'
);

CREATE TYPE notification.email_suppression_reason AS ENUM (
    'hard_bounce',
    'complaint',
    'provider_suppression',
    'manual'
);

CREATE TABLE notification.email_deliveries (
    id                       UUID                               NOT NULL PRIMARY KEY,
    store_id                 UUID                               NOT NULL,
    semantic_event_id        UUID                               NOT NULL,
    semantic_event_type      TEXT                               NOT NULL,
    recipient_email          extensions.citext                  NOT NULL,
    template_key             TEXT                               NOT NULL,
    template_version         INTEGER                            NOT NULL,
    template_payload         JSONB                              NOT NULL,
    provider                 TEXT                               NOT NULL DEFAULT 'resend',
    provider_message_id      TEXT,
    delivery_status          notification.email_delivery_status NOT NULL DEFAULT 'pending',
    attempts                 INTEGER                            NOT NULL DEFAULT 0,
    available_at             TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by                UUID,
    locked_at                TIMESTAMPTZ,
    sent_at                  TIMESTAMPTZ,
    delivered_at             TIMESTAMPTZ,
    last_error               TEXT,
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, semantic_event_id),
    UNIQUE (provider, provider_message_id),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT email_deliveries_semantic_event_type_check CHECK (
        semantic_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT email_deliveries_recipient_length_check CHECK (
        length(recipient_email::text) BETWEEN 3 AND 320
    ),
    CONSTRAINT email_deliveries_template_key_check CHECK (
        template_key ~ '^[a-z][a-z0-9_]{0,63}$'
    ),
    CONSTRAINT email_deliveries_template_version_check CHECK (template_version > 0),
    CONSTRAINT email_deliveries_template_payload_check CHECK (
        jsonb_typeof(template_payload) = 'object'
        AND octet_length(template_payload::text) <= 16384
    ),
    CONSTRAINT email_deliveries_provider_check CHECK (
        length(trim(provider)) BETWEEN 1 AND 50
    ),
    CONSTRAINT email_deliveries_provider_message_id_check CHECK (
        provider_message_id IS NULL OR length(provider_message_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT email_deliveries_attempts_check CHECK (attempts >= 0),
    CONSTRAINT email_deliveries_lease_check CHECK (
        (delivery_status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (delivery_status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    ),
    CONSTRAINT email_deliveries_sent_check CHECK (
        (delivery_status IN ('sent', 'delivered', 'bounced', 'complained')
            AND provider_message_id IS NOT NULL AND sent_at IS NOT NULL)
        OR delivery_status NOT IN ('sent', 'delivered', 'bounced', 'complained')
    ),
    CONSTRAINT email_deliveries_delivered_check CHECK (
        (delivery_status = 'delivered' AND delivered_at IS NOT NULL)
        OR (delivery_status <> 'delivered' AND delivered_at IS NULL)
    )
);

CREATE TABLE notification.email_suppressions (
    id                    UUID                                      NOT NULL PRIMARY KEY,
    store_id              UUID                                      NOT NULL,
    recipient_email       extensions.citext                         NOT NULL,
    suppression_reason    notification.email_suppression_reason     NOT NULL,
    source_delivery_id    UUID,
    created_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, recipient_email),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, source_delivery_id)
        REFERENCES notification.email_deliveries(store_id, id),
    CONSTRAINT email_suppressions_recipient_length_check CHECK (
        length(recipient_email::text) BETWEEN 3 AND 320
    )
);

CREATE TABLE notification.webhook_events (
    id                    UUID                     NOT NULL PRIMARY KEY,
    store_id              UUID                     NOT NULL,
    delivery_id           UUID                     NOT NULL,
    provider              TEXT                     NOT NULL,
    provider_event_id     TEXT                     NOT NULL,
    provider_event_type   TEXT                     NOT NULL,
    payload               JSONB                    NOT NULL,
    received_at           TIMESTAMPTZ              NOT NULL,
    processed_at          TIMESTAMPTZ,
    created_at            TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider, provider_event_id),
    FOREIGN KEY (store_id, delivery_id)
        REFERENCES notification.email_deliveries(store_id, id),
    CONSTRAINT notification_webhook_events_provider_check CHECK (
        length(trim(provider)) BETWEEN 1 AND 50
    ),
    CONSTRAINT notification_webhook_events_event_id_check CHECK (
        length(provider_event_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT notification_webhook_events_event_type_check CHECK (
        provider_event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT notification_webhook_events_payload_check CHECK (
        jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 65536
    )
);

CREATE INDEX email_deliveries_claim_idx
    ON notification.email_deliveries (delivery_status, available_at, created_at, id)
    WHERE delivery_status IN ('pending', 'processing');

CREATE INDEX email_deliveries_recipient_idx
    ON notification.email_deliveries (store_id,
        recipient_email,
        created_at DESC,
        id DESC
    );

CREATE INDEX notification_webhook_events_delivery_idx
    ON notification.webhook_events (store_id,
        delivery_id,
        received_at,
        id
    );

CREATE FUNCTION notification.email_delivery_metrics()
RETURNS TABLE (
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    suppressed BIGINT,
    oldest_pending_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (WHERE delivery.delivery_status = 'pending'),
           count(*) FILTER (WHERE delivery.delivery_status = 'processing'),
           count(*) FILTER (WHERE delivery.delivery_status = 'dead_letter'),
           count(*) FILTER (WHERE delivery.delivery_status = 'suppressed'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(delivery.created_at)
                            FILTER (WHERE delivery.delivery_status = 'pending'))
               ),
               0
           )::DOUBLE PRECISION
      FROM notification.email_deliveries AS delivery;
$$;

CREATE FUNCTION notification.claim_email_deliveries(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    recipient_email TEXT,
    template_key TEXT,
    template_version INTEGER,
    template_payload JSONB,
    provider TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH suppress AS (
        UPDATE notification.email_deliveries AS delivery
           SET delivery_status = 'suppressed',
               locked_by = NULL,
               locked_at = NULL,
               last_error = 'recipient is suppressed',
               updated_at = claimed_at
         WHERE delivery.delivery_status IN ('pending', 'processing')
           AND EXISTS (
               SELECT 1
                 FROM notification.email_suppressions AS suppression
                WHERE suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
        RETURNING delivery.id
    ), expired AS (
        UPDATE notification.email_deliveries AS delivery
           SET delivery_status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(delivery.last_error, 'worker lease expired after final attempt'),
               updated_at = claimed_at
         WHERE delivery.delivery_status = 'processing'
           AND delivery.locked_at <= stale_before
           AND delivery.attempts >= 8
        RETURNING delivery.id
    ), claimable AS (
        SELECT delivery.id
          FROM notification.email_deliveries AS delivery
         WHERE (
                 (delivery.delivery_status = 'pending' AND delivery.available_at <= claimed_at)
                 OR (delivery.delivery_status = 'processing' AND delivery.locked_at <= stale_before)
               )
           AND delivery.attempts < 8
           AND NOT EXISTS (
               SELECT 1
                 FROM notification.email_suppressions AS suppression
                WHERE suppression.store_id = delivery.store_id
                  AND suppression.recipient_email = delivery.recipient_email
           )
         ORDER BY delivery.available_at, delivery.created_at, delivery.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE notification.email_deliveries AS delivery
       SET delivery_status = 'processing',
           attempts = delivery.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at,
           updated_at = claimed_at
      FROM claimable
     WHERE delivery.id = claimable.id
    RETURNING delivery.id, delivery.store_id,
              delivery.recipient_email::text, delivery.template_key,
              delivery.template_version, delivery.template_payload,
              delivery.provider, delivery.attempts;
$$;

CREATE FUNCTION notification.finish_email_delivery(
    delivery_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    retryable BOOLEAN,
    provider_message_id TEXT,
    failure TEXT,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE notification.email_deliveries AS delivery
       SET delivery_status = CASE
               WHEN succeeded THEN 'sent'::notification.email_delivery_status
               WHEN NOT retryable THEN 'failed'::notification.email_delivery_status
               WHEN delivery.attempts >= 8 THEN 'dead_letter'::notification.email_delivery_status
               ELSE 'pending'::notification.email_delivery_status
           END,
           provider_message_id = CASE
               WHEN succeeded THEN $5 ELSE delivery.provider_message_id
           END,
           available_at = CASE
               WHEN succeeded OR NOT retryable THEN delivery.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(delivery.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           sent_at = CASE WHEN succeeded THEN finished_at ELSE delivery.sent_at END,
           last_error = CASE
               WHEN succeeded THEN NULL
               ELSE COALESCE(NULLIF(left(failure, 2000), ''), 'email delivery failed')
           END,
           updated_at = finished_at
     WHERE delivery.id = $1
       AND delivery.delivery_status = 'processing'
       AND delivery.locked_by = $2
    RETURNING true;
$$;

CREATE FUNCTION notification.record_resend_webhook(
    provider_event_id TEXT,
    provider_message_id TEXT,
    provider_event_type TEXT,
    payload JSONB,
    received_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    target notification.email_deliveries%ROWTYPE;
    webhook_id UUID;
    suppression_reason notification.email_suppression_reason;
BEGIN
    SELECT delivery.*
      INTO target
      FROM notification.email_deliveries AS delivery
     WHERE delivery.provider = 'resend'
       AND delivery.provider_message_id = record_resend_webhook.provider_message_id
     FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    webhook_id := uuidv7();
    INSERT INTO notification.webhook_events (
        id, store_id, delivery_id, provider, provider_event_id,
        provider_event_type, payload, received_at, processed_at
    ) VALUES (
        webhook_id, target.store_id, target.id, 'resend',
        record_resend_webhook.provider_event_id, record_resend_webhook.provider_event_type,
        payload, received_at, received_at
    ) ON CONFLICT ON CONSTRAINT webhook_events_provider_provider_event_id_key DO NOTHING;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    IF provider_event_type = 'email.sent'
       AND target.delivery_status IN ('pending', 'processing', 'sent') THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'sent', updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.delivered'
          AND target.delivery_status NOT IN ('bounced', 'complained', 'suppressed') THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'delivered', delivered_at = received_at, updated_at = received_at
         WHERE id = target.id;
    ELSIF provider_event_type = 'email.bounced' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'bounced', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status NOT IN ('complained', 'suppressed');
        IF lower(COALESCE(payload #>> '{data,bounce,type}', '')) = 'permanent' THEN
            suppression_reason := 'hard_bounce';
        END IF;
    ELSIF provider_event_type = 'email.complained' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'complained', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id AND delivery_status <> 'suppressed';
        suppression_reason := 'complaint';
    ELSIF provider_event_type = 'email.suppressed' THEN
        UPDATE notification.email_deliveries
           SET delivery_status = 'suppressed', delivered_at = NULL, updated_at = received_at
         WHERE id = target.id;
        suppression_reason := 'provider_suppression';
    END IF;

    IF suppression_reason IS NOT NULL THEN
        INSERT INTO notification.email_suppressions (
            id, store_id, recipient_email, suppression_reason,
            source_delivery_id, created_at, updated_at
        ) VALUES (
            uuidv7(), target.store_id, target.recipient_email,
            suppression_reason, target.id, received_at, received_at
        ) ON CONFLICT (store_id, recipient_email) DO UPDATE
            SET suppression_reason = CASE
                    WHEN notification.email_suppressions.suppression_reason = 'manual'
                        THEN notification.email_suppressions.suppression_reason
                    WHEN notification.email_suppressions.suppression_reason = 'complaint'
                        THEN notification.email_suppressions.suppression_reason
                    WHEN EXCLUDED.suppression_reason = 'complaint'
                        THEN EXCLUDED.suppression_reason
                    WHEN notification.email_suppressions.suppression_reason = 'hard_bounce'
                        THEN notification.email_suppressions.suppression_reason
                    ELSE EXCLUDED.suppression_reason
                END,
                source_delivery_id = CASE
                    WHEN notification.email_suppressions.suppression_reason IN ('manual', 'complaint')
                        THEN notification.email_suppressions.source_delivery_id
                    ELSE EXCLUDED.source_delivery_id
                END,
                updated_at = EXCLUDED.updated_at;
    END IF;
    RETURN true;
END;
$$;

ALTER TABLE notification.email_deliveries ENABLE ROW LEVEL SECURITY;

ALTER TABLE notification.email_suppressions ENABLE ROW LEVEL SECURITY;

ALTER TABLE notification.webhook_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON notification.email_deliveries
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON notification.email_suppressions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON notification.webhook_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION notification.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION notification.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION notification.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION notification.email_delivery_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION notification.claim_email_deliveries(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION notification.finish_email_delivery(
    UUID, UUID, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION notification.record_resend_webhook(
    TEXT, TEXT, TEXT, JSONB, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION notification.email_delivery_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA notification TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON notification.email_deliveries, notification.email_suppressions,
       notification.webhook_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA notification TO chaos_runtime;

GRANT USAGE ON SCHEMA notification, analytics TO chaos_runtime;
