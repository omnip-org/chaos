-- PGMQ message IDs are scoped to a queue. event_outbox routes several queues,
-- so pgmq_message_id must not be globally unique across the table.
ALTER TABLE integration.event_outbox
    ADD COLUMN queue_name TEXT;

UPDATE integration.event_outbox
   SET queue_name = integration.event_queue_name(event_type);

ALTER TABLE integration.event_outbox
    ALTER COLUMN queue_name SET NOT NULL,
    ADD CONSTRAINT event_outbox_queue_name_check CHECK (
        queue_name ~ '^chaos_[a-z][a-z0-9_]*$'
    );

ALTER TABLE integration.event_outbox
    DROP CONSTRAINT event_outbox_pgmq_message_id_key;

ALTER TABLE integration.event_outbox
    ADD CONSTRAINT event_outbox_queue_message_id_key
    UNIQUE (queue_name, pgmq_message_id);

CREATE OR REPLACE FUNCTION integration.enqueue_event_outbox()
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

CREATE OR REPLACE FUNCTION integration.claim_routed_event_outbox(
    queue_name TEXT,
    batch_size INTEGER
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    aggregate_id UUID,
    payload JSONB,
    occurred_at TIMESTAMPTZ,
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
    IF queue_name NOT IN (
        'chaos_payment_commands',
        'chaos_fulfillment_events',
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
        SELECT event.id,
               event.store_id,
               event.event_type,
               event.aggregate_id,
               event.payload,
               event.created_at
          INTO target
          FROM integration.event_outbox AS event
         WHERE event.queue_name = $1
           AND event.pgmq_message_id = message.msg_id
           AND event.processed_at IS NULL
           AND event.failed_at IS NULL;
        IF NOT FOUND THEN
            PERFORM pgmq.delete(queue_name, message.msg_id);
            CONTINUE;
        END IF;

        id := target.id;
        store_id := target.store_id;
        event_type := target.event_type;
        aggregate_id := target.aggregate_id;
        payload := target.payload;
        occurred_at := target.created_at;
        attempts := message.read_ct;
        RETURN NEXT;
    END LOOP;
END;
$$;
