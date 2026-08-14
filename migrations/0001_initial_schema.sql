CREATE SCHEMA extensions;
CREATE SCHEMA identity;
CREATE SCHEMA merchant;

COMMENT ON SCHEMA extensions IS 'PostgreSQL extension-owned objects';
COMMENT ON SCHEMA identity IS 'Users, credentials, service accounts, and sessions';
COMMENT ON SCHEMA merchant IS
    'Merchant accounts, memberships, stores, channels, and domains';

CREATE EXTENSION citext WITH SCHEMA extensions;

CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');
CREATE TYPE merchant.merchant_account_status AS ENUM ('active', 'suspended', 'closed');
CREATE TYPE merchant.merchant_role AS ENUM (
    'owner',
    'administrator',
    'developer',
    'manager',
    'support'
);
CREATE TYPE merchant.store_status AS ENUM ('draft', 'active', 'archived');

CREATE TABLE identity.users (
    id uuid PRIMARY KEY,
    email extensions.citext NOT NULL UNIQUE,
    status identity.user_status NOT NULL DEFAULT 'active',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    )
);

CREATE TABLE merchant.merchant_accounts (
    id uuid PRIMARY KEY,
    slug extensions.citext NOT NULL UNIQUE,
    display_name text NOT NULL,
    status merchant.merchant_account_status NOT NULL DEFAULT 'active',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT merchant_accounts_slug_format_check CHECK (
        slug::text ~ '^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$'
    ),
    CONSTRAINT merchant_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE merchant.merchant_account_memberships (
    merchant_account_id uuid NOT NULL
        REFERENCES merchant.merchant_accounts(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES identity.users(id) ON DELETE CASCADE,
    role merchant.merchant_role NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (merchant_account_id, user_id)
);

CREATE INDEX merchant_account_memberships_user_idx
    ON merchant.merchant_account_memberships (user_id, merchant_account_id);

CREATE TABLE merchant.stores (
    id uuid PRIMARY KEY,
    merchant_account_id uuid NOT NULL
        REFERENCES merchant.merchant_accounts(id),
    code extensions.citext NOT NULL,
    name text NOT NULL,
    default_currency char(3) NOT NULL,
    status merchant.store_status NOT NULL DEFAULT 'draft',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (merchant_account_id, code),
    UNIQUE (merchant_account_id, id),
    CONSTRAINT stores_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT stores_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT stores_currency_format_check CHECK (
        default_currency ~ '^[A-Z]{3}$'
    )
);

CREATE INDEX stores_merchant_account_status_idx
    ON merchant.stores (merchant_account_id, status);

CREATE TABLE merchant.store_currencies (
    merchant_account_id uuid NOT NULL,
    store_id uuid NOT NULL,
    currency char(3) NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    PRIMARY KEY (merchant_account_id, store_id, currency),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT store_currencies_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    )
);

CREATE TABLE merchant.store_domains (
    id uuid PRIMARY KEY,
    merchant_account_id uuid NOT NULL,
    store_id uuid NOT NULL,
    hostname extensions.citext NOT NULL UNIQUE,
    verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (merchant_account_id, store_id)
        REFERENCES merchant.stores(merchant_account_id, id) ON DELETE CASCADE,
    CONSTRAINT store_domains_hostname_lowercase_check CHECK (
        hostname::text = lower(hostname::text)
    )
);

CREATE INDEX store_domains_merchant_account_store_idx
    ON merchant.store_domains (merchant_account_id, store_id);

ALTER TABLE merchant.merchant_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.merchant_account_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.stores ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.store_currencies ENABLE ROW LEVEL SECURITY;
ALTER TABLE merchant.store_domains ENABLE ROW LEVEL SECURITY;

CREATE POLICY merchant_account_isolation ON merchant.merchant_accounts
    USING (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid);

CREATE POLICY merchant_account_isolation ON merchant.merchant_account_memberships
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.stores
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.store_currencies
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON merchant.store_domains
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

DO $$
BEGIN
    CREATE ROLE chaos_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    EXECUTE format('GRANT chaos_runtime TO %I', current_user);
END
$$;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

GRANT USAGE ON SCHEMA extensions, merchant TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA merchant TO chaos_runtime;
GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA merchant TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';
