-- Cart additions and checkout initiation are browser-owned events. Keep the
-- durable Meta CAPI queue subscribed only to confirmed Purchase events.
DELETE FROM chaos_integration.topic_bindings
WHERE queue_name = 'analytics_capi_queue'
  AND routing_key IN ('cart.item.added', 'order.payment.initiated');
