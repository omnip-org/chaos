-- Bind legacy Storefront Keys to each Store's active default Sales Channel.
UPDATE commerce.store_publishable_keys AS publishable_key
SET sales_channel_id = default_channel.id,
    updated_at = CURRENT_TIMESTAMP
FROM commerce.store_sales_channels AS default_channel
WHERE publishable_key.sales_channel_id IS NULL
  AND default_channel.store_id = publishable_key.store_id
  AND default_channel.is_default
  AND default_channel.status = 'active';

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM commerce.store_publishable_keys
        WHERE sales_channel_id IS NULL
    ) THEN
        RAISE EXCEPTION 'cannot bind every Publishable Key to an active default Sales Channel';
    END IF;
END
$$;

ALTER TABLE commerce.store_publishable_keys
    ALTER COLUMN sales_channel_id SET NOT NULL;

CREATE OR REPLACE FUNCTION commerce.authenticate_publishable_key (presented_public_key TEXT)
RETURNS TABLE (
    publishable_key_id UUID,
    store_id           UUID,
    sales_channel_id   UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT
        publishable_key.id        AS publishable_key_id,
        publishable_key.store_id,
        sales_channel.id          AS sales_channel_id
    FROM
        commerce.store_publishable_keys AS publishable_key
        INNER JOIN commerce.stores AS store
            ON store.id = publishable_key.store_id
        INNER JOIN commerce.store_sales_channels AS sales_channel
            ON sales_channel.store_id = publishable_key.store_id
            AND sales_channel.id = publishable_key.sales_channel_id
            AND sales_channel.status = 'active'
    WHERE
        publishable_key.public_key = presented_public_key
        AND publishable_key.revoked_at IS NULL
        AND store.status = 'active';
$$;
