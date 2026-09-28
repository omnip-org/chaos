CREATE FUNCTION chaos_commerce.resolve_price_list (
    p_store_id UUID,
    p_currency CHAR(3),
    p_at TIMESTAMPTZ
)
RETURNS TABLE (id UUID, currency CHAR(3))
LANGUAGE sql
STABLE
AS $$
    SELECT price_list.id, price_list.currency
    FROM chaos_commerce.price_lists AS price_list
    INNER JOIN chaos_commerce.stores AS store
      ON store.id = price_list.store_id
    WHERE price_list.store_id = p_store_id
      AND price_list.status = 'active'
      AND store.status = 'active'
      AND price_list.currency = p_currency
      AND (price_list.starts_at IS NULL OR price_list.starts_at <= p_at)
      AND (price_list.ends_at IS NULL OR price_list.ends_at > p_at)
    ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC
    LIMIT 1
$$;

REVOKE ALL ON FUNCTION chaos_commerce.resolve_price_list (UUID, CHAR(3), TIMESTAMPTZ) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION chaos_commerce.resolve_price_list (UUID, CHAR(3), TIMESTAMPTZ) TO chaos_runtime;
