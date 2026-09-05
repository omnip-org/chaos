-- Replace the analytics/CAPI delivery fan-out (analytics_deliveries +
-- schedule_analytics_deliveries's cursor scan) and the single-consumer
-- `search.product.changed`/`order.confirmed` event_outbox routes with
-- native PGMQ topic routing (pgmq.bind_topic/pgmq.send_topic). Each
-- consumer gets its own queue and is fully isolated: no shared
-- cross-consumer completion state, no bespoke delivery/audit table. PGMQ's
-- own message lifecycle (visibility timeout retry, archive() on exhausted
-- retries) is the only durability relied on; failures are logged by the
-- consuming worker, not persisted.

SELECT pgmq.create('search_index_queue');
SELECT pgmq.create('analytics_capi_queue');
SELECT pgmq.create('notification_email_queue');

SELECT pgmq.bind_topic('product.updated',   'search_index_queue');
SELECT pgmq.bind_topic('payment.initiated', 'analytics_capi_queue');
SELECT pgmq.bind_topic('payment.completed', 'analytics_capi_queue');
SELECT pgmq.bind_topic('payment.completed', 'notification_email_queue');
-- A manual admin order confirmation (PostgresOrderManagementRepository::
-- transition_order) has no captured payment or analytics event behind it,
-- so it only ever notifies email, never CAPI.
SELECT pgmq.bind_topic('order.confirmed',   'notification_email_queue');

-- One narrow entry point into the pgmq schema for producers, mirroring why
-- enqueue_event_outbox()/enqueue_webhook_event() are SECURITY DEFINER today.
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
-- every topic-routed consumer below (mirrors claim_event_outbox's existing
-- reuse across chaos_payment_commands/chaos_email_commands/chaos_shipping_commands).
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

REVOKE ALL ON FUNCTION integration.publish_commerce_event (TEXT, JSONB) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.claim_topic_queue (TEXT, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION integration.finish_topic_event (TEXT, BIGINT, INTEGER, BOOLEAN, INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION integration.publish_commerce_event (TEXT, JSONB) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.claim_topic_queue (TEXT, INTEGER) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION integration.finish_topic_event (TEXT, BIGINT, INTEGER, BOOLEAN, INTEGER) TO chaos_runtime;

-- Retire the analytics_deliveries fan-out: it only ever served one
-- destination (meta), so the delivery-row table + cursor scan that let one
-- event fan out to N destinations was unused generality. Topic routing
-- replaces it: analytics_events' insert now publishes 'payment.initiated'/
-- 'payment.completed' directly (see the application-layer changes),
-- delivered immediately instead of on the next scheduled scan.
DROP TRIGGER analytics_deliveries_enqueue ON integration.analytics_deliveries;
DROP FUNCTION integration.enqueue_analytics_event_delivery ();
DROP FUNCTION integration.finish_analytics_event_delivery (UUID, INTEGER, INTEGER, BOOLEAN, BOOLEAN, TEXT, TEXT, TIMESTAMPTZ);
DROP FUNCTION integration.claim_analytics_deliveries (INTEGER);
DROP FUNCTION integration.schedule_analytics_deliveries (INTEGER);
DROP TABLE integration.analytics_deliveries;
DROP TYPE integration.delivery_status;
SELECT pgmq.drop_queue('chaos_analytics_deliveries');

-- analytics_destinations no longer needs a delivery-scheduling cursor: there
-- is no scan to resume, publishing happens inline with the event insert.
CREATE OR REPLACE FUNCTION integration.configure_analytics_destination (
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

ALTER TABLE integration.analytics_destinations
    DROP COLUMN schedule_cursor_received_at,
    DROP COLUMN schedule_cursor_event_id;

-- Retire the migrated event_outbox producers (product/variant change
-- triggers below, plus the Rust call sites in payments/events.rs and
-- sales/order_management.rs). The event_routes rows for
-- 'search.product.changed'/'order.confirmed' are left in place (historical
-- event_outbox rows still carry an FK to them); nothing writes those event
-- types anymore so they have no further runtime effect.
DROP INDEX integration.event_outbox_pending_search_product_key_idx;

CREATE OR REPLACE FUNCTION commerce.capture_product_change ()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    PERFORM integration.publish_commerce_event(
        'product.updated',
        jsonb_build_object('store_id', NEW.store_id, 'product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION commerce.capture_variant_change ()
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
        PERFORM integration.publish_commerce_event(
            'product.updated',
            jsonb_build_object('store_id', owning_store_id, 'product_id', changed_product_id)
        );
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION commerce.process_events (
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
        SELECT queued.msg_id, queued.payload, queued.attempts
        FROM integration.claim_topic_queue('search_index_queue', batch_size) AS queued
    LOOP
        BEGIN
            PERFORM commerce.refresh_product_document(
                (event.payload->>'store_id')::uuid,
                (event.payload->>'product_id')::uuid
            );
            PERFORM integration.finish_topic_event(
                'search_index_queue', event.msg_id, event.attempts, true, max_attempts
            );
            processed := processed + 1;
        EXCEPTION WHEN OTHERS THEN
            PERFORM integration.finish_topic_event(
                'search_index_queue', event.msg_id, event.attempts, false, max_attempts
            );
        END;
    END LOOP;

    RETURN processed;
END;
$$;

-- These two queues are now orphaned: every producer that used to route
-- through them ('order.confirmed'/'search.product.changed' event_outbox
-- rows) publishes to the topic-routed queues above instead.
SELECT pgmq.drop_queue('chaos_email_commands');
SELECT pgmq.drop_queue('chaos_search_events');
