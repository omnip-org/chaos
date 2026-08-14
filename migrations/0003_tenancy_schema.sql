CREATE SCHEMA IF NOT EXISTS extensions;
CREATE SCHEMA IF NOT EXISTS tenancy;

COMMENT ON SCHEMA extensions IS 'PostgreSQL extension-owned objects';
COMMENT ON SCHEMA tenancy IS 'Tenant, store, membership, role, channel, and domain ownership';

ALTER EXTENSION citext SET SCHEMA extensions;

ALTER TYPE public.tenant_status SET SCHEMA tenancy;
ALTER TYPE public.store_status SET SCHEMA tenancy;

ALTER TABLE public.tenants SET SCHEMA tenancy;
ALTER TABLE public.stores SET SCHEMA tenancy;
ALTER TABLE public.store_currencies SET SCHEMA tenancy;
ALTER TABLE public.tenant_domains SET SCHEMA tenancy;

ALTER TABLE tenancy.tenants
    RENAME CONSTRAINT tenants_slug_check TO tenants_slug_format_check;
ALTER TABLE tenancy.stores
    RENAME CONSTRAINT stores_default_currency_check TO stores_default_currency_format_check;
ALTER TABLE tenancy.store_currencies
    RENAME CONSTRAINT store_currencies_currency_check TO store_currencies_currency_format_check;
ALTER TABLE tenancy.tenant_domains
    RENAME CONSTRAINT tenant_domains_hostname_check TO tenant_domains_hostname_lowercase_check;

ALTER POLICY tenant_isolation_stores ON tenancy.stores
    RENAME TO tenant_isolation;
ALTER POLICY tenant_isolation_store_currencies ON tenancy.store_currencies
    RENAME TO tenant_isolation;
ALTER POLICY tenant_isolation_tenant_domains ON tenancy.tenant_domains
    RENAME TO tenant_isolation;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

GRANT USAGE ON SCHEMA extensions, tenancy TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA tenancy TO chaos_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA tenancy TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA tenancy
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA tenancy
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
