DROP FUNCTION merchant.authenticate_api_key(TEXT, BYTEA);

CREATE FUNCTION merchant.authenticate_api_key(
    presented_key_identifier  TEXT,
    presented_secret_digest  BYTEA
)
RETURNS TABLE (
    api_key_id           UUID,
    merchant_account_id  UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    class                TEXT,
    mode                 TEXT,
    scopes               TEXT[],
    created_by_user_id   UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT api_key.id,
           api_key.merchant_account_id,
           api_key.store_id,
           sales_channel.id,
           api_key.class::TEXT,
           api_key.mode::TEXT,
           ARRAY(
               SELECT api_key_scope.scope::TEXT
               FROM merchant.api_key_scopes AS api_key_scope
               WHERE api_key_scope.merchant_account_id = api_key.merchant_account_id
                 AND api_key_scope.api_key_id = api_key.id
               ORDER BY api_key_scope.scope::TEXT
           ),
           api_key.created_by_user_id
    FROM merchant.api_keys AS api_key
    INNER JOIN merchant.merchant_accounts AS merchant_account
        ON merchant_account.id = api_key.merchant_account_id
    INNER JOIN merchant.stores AS store
        ON store.merchant_account_id = api_key.merchant_account_id
       AND store.id = api_key.store_id
    LEFT JOIN merchant.sales_channels AS sales_channel
        ON sales_channel.merchant_account_id = api_key.merchant_account_id
       AND sales_channel.store_id = api_key.store_id
       AND sales_channel.id = COALESCE(
           api_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM merchant.sales_channels AS default_channel
               WHERE default_channel.merchant_account_id = api_key.merchant_account_id
                 AND default_channel.store_id = api_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE api_key.key_identifier = presented_key_identifier
      AND api_key.secret_digest = presented_secret_digest
      AND api_key.revoked_at IS NULL
      AND (api_key.expires_at IS NULL OR api_key.expires_at > CURRENT_TIMESTAMP)
      AND merchant_account.status = 'active'
      AND (api_key.mode = 'test' OR store.status = 'active');
$$;
