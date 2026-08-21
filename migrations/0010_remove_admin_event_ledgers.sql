-- === Remove unused administrative event ledgers ===
--
-- These tables recorded write-side history without any runtime reader. The
-- transactional rows remain the source of truth for current business state;
-- order transitions and fulfillment transitions are intentionally retained
-- because the business APIs use them as state history and idempotency evidence.

DROP TABLE IF EXISTS commerce.store_locale_events CASCADE;
DROP TABLE IF EXISTS commerce.product_translation_events CASCADE;
DROP TABLE IF EXISTS commerce.collection_translation_events CASCADE;
DROP TABLE IF EXISTS commerce.media_translation_events CASCADE;
DROP TABLE IF EXISTS commerce.media_events CASCADE;
DROP TABLE IF EXISTS commerce.collection_events CASCADE;
DROP TABLE IF EXISTS commerce.review_events CASCADE;

DROP TYPE IF EXISTS commerce.store_locale_event_kind;
DROP TYPE IF EXISTS commerce.translation_event_kind;
DROP TYPE IF EXISTS commerce.collection_event_kind;
DROP TYPE IF EXISTS commerce.media_event_kind;
DROP TYPE IF EXISTS commerce.review_event_kind;
