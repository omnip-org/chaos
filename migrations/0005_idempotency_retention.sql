-- Idempotency records are replay state, not an audit history. Keep them long
-- enough for client retries, then remove them in bounded batches.
ALTER TABLE integration.idempotency_keys
    ADD COLUMN expires_at TIMESTAMPTZ NOT NULL
        DEFAULT (CURRENT_TIMESTAMP + INTERVAL '7 days');

CREATE INDEX idempotency_keys_expiry_idx
    ON integration.idempotency_keys (expires_at)
    WHERE completed_at IS NOT NULL;

SELECT cron.schedule(
    'chaos-idempotency-cleanup',
    '17 * * * *',
    $$
    DELETE FROM integration.idempotency_keys
     WHERE id IN (
         SELECT id
           FROM integration.idempotency_keys
          WHERE completed_at IS NOT NULL
            AND expires_at < CURRENT_TIMESTAMP
          ORDER BY expires_at, id
          LIMIT 10000
     )
    $$
);

COMMENT ON COLUMN integration.idempotency_keys.expires_at IS
    'After this time the idempotency replay snapshot may be removed; default retention is seven days.';
