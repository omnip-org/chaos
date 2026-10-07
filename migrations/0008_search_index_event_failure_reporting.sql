-- Return per-message failures to the application so retry and terminal
-- archive outcomes are visible in worker logs. The original function remains
-- available for compatibility with an older worker during a rolling deploy.
CREATE FUNCTION chaos_commerce.process_search_index_events_detailed (
    batch_size    INTEGER,
    max_attempts  INTEGER,
    finished_at   TIMESTAMPTZ
)
RETURNS TABLE (
    processed_count BIGINT,
    failures        JSONB
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    event           RECORD;
    failure_message TEXT;
    archived        BOOLEAN;
BEGIN
    processed_count := 0;
    failures := '[]'::jsonb;

    FOR event IN
        SELECT queued.msg_id, queued.payload, queued.attempts
        FROM chaos_integration.claim_topic_queue('search_index_queue', batch_size) AS queued
    LOOP
        BEGIN
            PERFORM chaos_commerce.refresh_product_document(
                (event.payload->>'store_id')::uuid,
                (event.payload->>'product_id')::uuid,
                finished_at
            );
            PERFORM chaos_integration.finish_topic_event(
                'search_index_queue', event.msg_id, event.attempts, true, max_attempts
            );
            processed_count := processed_count + 1;
        EXCEPTION WHEN OTHERS THEN
            GET STACKED DIAGNOSTICS failure_message = MESSAGE_TEXT;
            archived := event.attempts >= greatest(max_attempts, 1);
            PERFORM chaos_integration.finish_topic_event(
                'search_index_queue', event.msg_id, event.attempts, false, max_attempts
            );
            failures := failures || jsonb_build_array(jsonb_build_object(
                'msg_id', event.msg_id,
                'attempts', event.attempts,
                'archived', archived,
                'error', failure_message,
                'store_id', event.payload->>'store_id',
                'product_id', event.payload->>'product_id'
            ));
        END;
    END LOOP;

    RETURN NEXT;
END;
$$;

REVOKE ALL ON FUNCTION chaos_commerce.process_search_index_events_detailed (INTEGER, INTEGER, TIMESTAMPTZ) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION chaos_commerce.process_search_index_events_detailed (INTEGER, INTEGER, TIMESTAMPTZ) TO chaos_runtime;
