CREATE EXTENSION IF NOT EXISTS citext;

CREATE TYPE tenant_status AS ENUM ('active', 'suspended', 'closed');
CREATE TYPE store_status AS ENUM ('draft', 'active', 'archived');

CREATE TABLE tenants (
    id uuid PRIMARY KEY,
    slug citext NOT NULL UNIQUE,
    display_name text NOT NULL,
    status tenant_status NOT NULL DEFAULT 'active',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$')
);

CREATE TABLE stores (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    code citext NOT NULL,
    name text NOT NULL,
    default_currency char(3) NOT NULL,
    status store_status NOT NULL DEFAULT 'draft',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, code),
    UNIQUE (tenant_id, id),
    CHECK (default_currency ~ '^[A-Z]{3}$')
);

CREATE TABLE store_currencies (
    tenant_id uuid NOT NULL,
    store_id uuid NOT NULL,
    currency char(3) NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    PRIMARY KEY (tenant_id, store_id, currency),
    FOREIGN KEY (tenant_id, store_id) REFERENCES stores(tenant_id, id),
    CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE tenant_domains (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    store_id uuid NOT NULL,
    hostname citext NOT NULL UNIQUE,
    verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, store_id) REFERENCES stores(tenant_id, id),
    CHECK (hostname = lower(hostname))
);

ALTER TABLE stores ENABLE ROW LEVEL SECURITY;
ALTER TABLE store_currencies ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_domains ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation_stores ON stores
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY tenant_isolation_store_currencies ON store_currencies
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY tenant_isolation_tenant_domains ON tenant_domains
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE INDEX stores_tenant_status_idx ON stores (tenant_id, status);
CREATE INDEX tenant_domains_tenant_store_idx ON tenant_domains (tenant_id, store_id);
