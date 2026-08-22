-- Analytics destinations are mutable Store configuration, but the runtime
-- role must not receive direct INSERT, UPDATE, or DELETE privileges on the
-- configuration table. Route the controlled upsert through a Store-scoped
-- function.
CREATE FUNCTION integration.configure_analytics_destination(
    p_store_id UUID,
    p_provider TEXT,
    p_external_account_reference TEXT,
    p_credential_secret_reference TEXT,
    p_configuration JSONB,
    p_enabled BOOLEAN,
    p_created_by UUID,
    p_now TIMESTAMPTZ
)
RETURNS TABLE (
    destination_id UUID,
    destination_provider TEXT,
    destination_external_account_reference TEXT,
    destination_configuration JSONB,
    destination_enabled BOOLEAN,
    destination_created_at TIMESTAMPTZ,
    destination_updated_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF p_store_id IS DISTINCT FROM
       nullif(current_setting('app.store_id', true), '')::uuid
    THEN
        RAISE EXCEPTION 'analytics destination store context does not match target store'
            USING ERRCODE = '42501';
    END IF;

    IF p_created_by IS DISTINCT FROM
       nullif(current_setting('app.user_id', true), '')::uuid
    THEN
        RAISE EXCEPTION 'analytics destination user context does not match creator'
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
        created_by,
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
        p_created_by,
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

REVOKE ALL ON FUNCTION integration.configure_analytics_destination(
    UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.configure_analytics_destination(
    UUID, TEXT, TEXT, TEXT, JSONB, BOOLEAN, UUID, TIMESTAMPTZ
) TO chaos_runtime;

-- The delivery table is also workflow-owned: scheduling, claiming, and
-- completion are the only supported write paths. Keeping direct writes away
-- from the runtime role prevents a worker or future repository from creating
-- rows without the PGMQ message and delivery-state invariants.
REVOKE INSERT, UPDATE, DELETE
    ON integration.analytics_destinations,
       integration.analytics_deliveries
    FROM chaos_runtime;
