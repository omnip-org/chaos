REVOKE CREATE ON SCHEMA public FROM PUBLIC;

-- Later capability migrations share physical schemas and therefore repeat broad
-- grants. Reassert append-only and server-managed table permissions once after
-- the complete bootstrap has been installed.
REVOKE UPDATE, DELETE
    ON commerce.store_locale_events,
       commerce.collection_events,
       commerce.collection_translation_events,
       commerce.media_events,
       commerce.media_translation_events,
       commerce.product_translation_events,
       commerce.review_events,
       commerce.stock_ledger_entries,
       commerce.customer_shopper_links,
       commerce.checkout_contacts,
       commerce.checkout_addresses,
       commerce.checkout_lines,
       commerce.checkout_tax_calculations,
       commerce.checkout_promotion_calculations,
       commerce.checkout_shipping_selections,
       commerce.order_contacts,
       commerce.order_addresses,
       commerce.order_lines,
       commerce.order_tax_calculations,
       commerce.order_promotion_calculations,
       commerce.order_shipping_selections,
       commerce.order_transitions,
       commerce.order_fulfillment_transitions
    FROM chaos_runtime;

REVOKE DELETE
    ON commerce.collections,
       commerce.media_assets,
       commerce.reviews,
       commerce.checkouts,
       commerce.orders
    FROM chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON integration.event_consumer_registry FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.webhook_inbox,
       integration.outbox_events,
       integration.email_deliveries,
       integration.email_suppressions,
       integration.webhook_events,
       integration.store_policy_versions,
       integration.identity_links,
       integration.behavior_events,
       integration.erasure_requests,
       integration.commerce_facts
    FROM chaos_runtime;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';

COMMENT ON ROLE chaos_identity IS
    'Non-owner identity role. It cannot access Store-owned commerce tables.';
