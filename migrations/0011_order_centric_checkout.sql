BEGIN;

ALTER TABLE commerce.carts
    ADD COLUMN payment_client_action JSONB;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM commerce.checkout_attempts
        WHERE (provider_public_key IS NULL) <> (provider_client_secret IS NULL)
    ) THEN
        RAISE EXCEPTION 'checkout_attempts contains a partial payment client action';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM commerce.orders
        GROUP BY store_id, cart_id
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'more than one Order is attached to the same Cart';
    END IF;
END
$$;

-- The client handoff is the only payment-session data that must survive a
-- request retry. Provider object IDs, return URLs, policy snapshots, and
-- expiry are either derivable, request-scoped, or owned by the provider.
UPDATE commerce.carts AS cart
   SET payment_client_action = jsonb_build_object(
           'type', 'mount_embedded_checkout',
           'public_key', attempt.provider_public_key,
           'client_token', attempt.provider_client_secret
       )
  FROM commerce.checkout_attempts AS attempt
 WHERE attempt.store_id = cart.store_id
   AND attempt.source_cart_id = cart.id
   AND attempt.provider_public_key IS NOT NULL
   AND attempt.provider_client_secret IS NOT NULL;

-- A terminal Order must never expose a stale payment handoff during the
-- migration. Pending Orders keep their action for resume.
UPDATE commerce.carts AS cart
   SET payment_client_action = NULL
  FROM commerce.orders AS sales_order
 WHERE sales_order.store_id = cart.store_id
   AND sales_order.cart_id = cart.id
   AND (sales_order.status <> 'pending' OR sales_order.payment_status <> 'pending');

-- The previous fingerprint also included the Cart version, line snapshot, and
-- shipping-policy snapshot. Those inputs are deliberately no longer part of
-- the retry contract, so historical Orders cannot be compared to the new
-- request fingerprint format. A pending legacy Order remains resumable by
-- its existing idempotency key; new attempts write the v3 fingerprint.
UPDATE commerce.orders
   SET checkout_request_fingerprint = NULL
 WHERE checkout_request_fingerprint IS NOT NULL;

-- Historical versions could leave more than one active Cart for a shopper.
-- Keep the newest one as the canonical session and retain older rows as
-- abandoned records instead of deleting their line snapshots.
WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY store_id, sales_channel_id, shopper_id
               ORDER BY updated_at DESC, id DESC
           ) AS position
    FROM commerce.carts
    WHERE status = 'active'
)
UPDATE commerce.carts AS cart
   SET status = 'abandoned',
       version = version + 1,
       updated_at = CURRENT_TIMESTAMP
  FROM ranked
 WHERE ranked.id = cart.id
   AND ranked.position > 1;

CREATE TYPE commerce.cart_status_order_centric AS ENUM (
    'active',
    'locked',
    'abandoned'
);

ALTER TABLE commerce.carts
    ALTER COLUMN status DROP DEFAULT;

ALTER TABLE commerce.carts
    ALTER COLUMN status TYPE commerce.cart_status_order_centric
    USING (
        CASE status::text
            WHEN 'active' THEN 'active'
            WHEN 'completed' THEN 'locked'
            WHEN 'checkout_pending' THEN 'locked'
            WHEN 'abandoned' THEN 'abandoned'
        END
    )::commerce.cart_status_order_centric;

DROP TYPE commerce.cart_status;
ALTER TYPE commerce.cart_status_order_centric RENAME TO cart_status;

ALTER TABLE commerce.carts
    ALTER COLUMN status SET DEFAULT 'active';

ALTER TABLE commerce.carts
    ADD CONSTRAINT carts_payment_client_action_check
    CHECK (
        payment_client_action IS NULL
        OR (
            status = 'locked'
            AND
            jsonb_typeof(payment_client_action) = 'object'
            AND payment_client_action ? 'type'
            AND jsonb_typeof(payment_client_action->'type') = 'string'
            AND pg_column_size(payment_client_action) <= 8192
        )
    );

CREATE UNIQUE INDEX carts_one_active_per_shopper_key
    ON commerce.carts (store_id, sales_channel_id, shopper_id)
    WHERE status = 'active';

DROP INDEX commerce.orders_store_cart_idx;
CREATE UNIQUE INDEX orders_one_order_per_cart_key
    ON commerce.orders (store_id, cart_id);

DROP FUNCTION commerce.expire_checkout_attempts(INTEGER);
DROP TABLE commerce.checkout_attempts;
DROP TYPE commerce.checkout_attempt_status;

COMMIT;
