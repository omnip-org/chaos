-- First post-bootstrap contract migration.
--
-- Guest Order access moves from the long-lived `ot_` tracking capability to a
-- direct order-number + email lookup served by `POST /api/v1/orders/lookup`.
-- The capability table and its Worker cleanup routine are no longer written or
-- read by the application, so they are dropped here. This is a destructive drop
-- of a small, now-unused table: deploy the tracking-free application before
-- running this migration.

DROP FUNCTION IF EXISTS commerce.cleanup_expired_order_tracking_tokens(INTEGER);

DROP TABLE IF EXISTS commerce.order_tracking_tokens;
