CREATE SCHEMA extensions;

CREATE SCHEMA identity;

CREATE SCHEMA integration;

CREATE SCHEMA merchant;

CREATE SCHEMA catalog;

CREATE SCHEMA pricing;

CREATE SCHEMA inventory;

CREATE SCHEMA sales;

CREATE SCHEMA payments;

CREATE SCHEMA fulfillment;

CREATE SCHEMA notification;

CREATE SCHEMA analytics;

CREATE SCHEMA search;

CREATE SCHEMA partman;

COMMENT ON SCHEMA extensions IS 'PostgreSQL extension-owned objects';

COMMENT ON SCHEMA identity IS 'Users, credentials, service accounts, and sessions';

COMMENT ON SCHEMA integration IS
    'Idempotency records, webhooks, outbox delivery, and external mappings';

COMMENT ON SCHEMA merchant IS
    'Merchant accounts, memberships, stores, and channels';

COMMENT ON SCHEMA catalog IS
    'Products, variants, options, collections, media, and channel publication';

COMMENT ON SCHEMA pricing IS
    'Price lists, currency-specific prices, promotions, and tax classes';

COMMENT ON SCHEMA inventory IS
    'Locations, stock balances, append-only ledger entries, and reservations';

COMMENT ON SCHEMA sales IS
    'Carts, immutable checkout calculations, orders, returns, and exchanges';

COMMENT ON SCHEMA payments IS
    'Provider accounts, payment attempts, captures, and refunds';

COMMENT ON SCHEMA fulfillment IS
    'Partial fulfillments, shipments, tracking, and return logistics';

COMMENT ON SCHEMA notification IS
    'Semantic delivery requests, recipient policy, suppression, and delivery status';

COMMENT ON SCHEMA analytics IS
    'Canonical behavior events, consent evidence, attribution, and analytical delivery state';

COMMENT ON SCHEMA search IS
    'Rebuildable Store-isolated read models for storefront discovery';

COMMENT ON SCHEMA partman IS 'Objects owned by the pg_partman extension';

CREATE EXTENSION citext WITH SCHEMA extensions;

CREATE EXTENSION IF NOT EXISTS pg_partman WITH SCHEMA partman;

CREATE EXTENSION IF NOT EXISTS pg_cron;

CREATE EXTENSION IF NOT EXISTS pgmq;

DO $$
BEGIN
    CREATE ROLE chaos_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    CREATE ROLE chaos_control_plane NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    EXECUTE format('GRANT chaos_runtime TO %I', current_user);
    EXECUTE format('GRANT chaos_control_plane TO %I', current_user);
END
$$;
