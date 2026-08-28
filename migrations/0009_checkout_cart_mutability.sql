-- A Cart remains active while a Checkout Order is pending, so a shopper may
-- edit it and create a new Order snapshot with a different idempotency key.
-- Remove the temporary one-pending-per-Cart index from databases that already
-- applied the earlier bootstrap migration.
DROP INDEX IF EXISTS commerce.orders_one_pending_per_cart_idx;
