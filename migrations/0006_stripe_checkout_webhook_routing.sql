-- === Direct Stripe Checkout Webhook routing ===

-- Provider Account UUIDs are the only trusted routing identity for payment
-- webhooks. The old external reference was a Connect-era routing surrogate.
DROP FUNCTION commerce.resolve_provider_account(TEXT, TEXT);
DROP FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, TEXT);
DROP FUNCTION commerce.claim_provider_readiness_checks(UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM commerce.provider_accounts
         WHERE provider = 'stripe'
    ) THEN
        RAISE EXCEPTION
            'cannot migrate legacy stripe PaymentIntent accounts automatically; remove or archive them before applying 0006';
    END IF;
END;
$$;

ALTER TABLE integration.webhook_inbox
    ADD COLUMN provider_account_id UUID;

UPDATE integration.webhook_inbox AS event
   SET provider_account_id = account.id
  FROM commerce.provider_accounts AS account
 WHERE account.store_id = event.store_id
   AND account.provider = event.provider
   AND account.external_account_reference = event.external_account_reference;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM integration.webhook_inbox
         WHERE provider_account_id IS NULL
    ) THEN
        RAISE EXCEPTION 'cannot route existing payment webhooks to a Provider Account';
    END IF;
END;
$$;

ALTER TABLE integration.webhook_inbox
    ALTER COLUMN provider_account_id SET NOT NULL;

ALTER TABLE integration.webhook_inbox
    DROP CONSTRAINT webhook_inbox_provider_provider_event_id_key,
    DROP COLUMN external_account_reference,
    ADD CONSTRAINT webhook_inbox_store_provider_account_fkey
        FOREIGN KEY (store_id, provider_account_id)
        REFERENCES commerce.provider_accounts(store_id, id),
    ADD CONSTRAINT webhook_inbox_provider_account_event_key
        UNIQUE (provider_account_id, provider_event_id);

ALTER TABLE commerce.provider_accounts
    DROP CONSTRAINT provider_accounts_provider_external_account_reference_key,
    DROP COLUMN external_account_reference;

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
      FROM commerce.provider_accounts AS account
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
      FROM commerce.provider_accounts AS account
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
        UPDATE commerce.provider_accounts AS account
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
          FROM commerce.provider_accounts AS account
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
    UPDATE commerce.provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID)
    TO chaos_runtime;
GRANT EXECUTE ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

CREATE INDEX webhook_inbox_provider_account_idx
    ON integration.webhook_inbox (provider_account_id, created_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;
