ALTER SCHEMA tenancy RENAME TO merchant;
COMMENT ON SCHEMA merchant IS
    'Merchant accounts, memberships, roles, stores, channels, and domains';

ALTER TYPE merchant.tenant_status RENAME TO merchant_account_status;

ALTER TABLE merchant.tenants RENAME TO merchant_accounts;
ALTER TABLE merchant.tenant_domains RENAME TO store_domains;

ALTER TABLE merchant.stores
    RENAME COLUMN tenant_id TO merchant_account_id;
ALTER TABLE merchant.store_currencies
    RENAME COLUMN tenant_id TO merchant_account_id;
ALTER TABLE merchant.store_domains
    RENAME COLUMN tenant_id TO merchant_account_id;

ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_pkey TO merchant_accounts_pkey;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_slug_key TO merchant_accounts_slug_key;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_slug_format_check TO merchant_accounts_slug_format_check;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_id_not_null TO merchant_accounts_id_not_null;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_slug_not_null TO merchant_accounts_slug_not_null;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_display_name_not_null TO merchant_accounts_display_name_not_null;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_status_not_null TO merchant_accounts_status_not_null;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_created_at_not_null TO merchant_accounts_created_at_not_null;
ALTER TABLE merchant.merchant_accounts
    RENAME CONSTRAINT tenants_updated_at_not_null TO merchant_accounts_updated_at_not_null;

ALTER TABLE merchant.stores
    RENAME CONSTRAINT stores_tenant_id_fkey TO stores_merchant_account_id_fkey;
ALTER TABLE merchant.stores
    RENAME CONSTRAINT stores_tenant_id_code_key TO stores_merchant_account_id_code_key;
ALTER TABLE merchant.stores
    RENAME CONSTRAINT stores_tenant_id_id_key TO stores_merchant_account_id_id_key;
ALTER TABLE merchant.stores
    RENAME CONSTRAINT stores_tenant_id_not_null TO stores_merchant_account_id_not_null;

ALTER TABLE merchant.store_currencies
    RENAME CONSTRAINT store_currencies_tenant_id_store_id_fkey
    TO store_currencies_merchant_account_id_store_id_fkey;
ALTER TABLE merchant.store_currencies
    RENAME CONSTRAINT store_currencies_tenant_id_not_null
    TO store_currencies_merchant_account_id_not_null;

ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_pkey TO store_domains_pkey;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_hostname_key TO store_domains_hostname_key;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_hostname_lowercase_check
    TO store_domains_hostname_lowercase_check;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_tenant_id_fkey
    TO store_domains_merchant_account_id_fkey;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_tenant_id_store_id_fkey
    TO store_domains_merchant_account_id_store_id_fkey;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_id_not_null TO store_domains_id_not_null;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_tenant_id_not_null
    TO store_domains_merchant_account_id_not_null;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_store_id_not_null TO store_domains_store_id_not_null;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_hostname_not_null TO store_domains_hostname_not_null;
ALTER TABLE merchant.store_domains
    RENAME CONSTRAINT tenant_domains_created_at_not_null TO store_domains_created_at_not_null;

ALTER INDEX merchant.stores_tenant_status_idx
    RENAME TO stores_merchant_account_status_idx;
ALTER INDEX merchant.tenant_domains_tenant_store_idx
    RENAME TO store_domains_merchant_account_store_idx;

DROP POLICY tenant_isolation ON merchant.stores;
DROP POLICY tenant_isolation ON merchant.store_currencies;
DROP POLICY tenant_isolation ON merchant.store_domains;

ALTER TABLE merchant.merchant_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY merchant_account_isolation ON merchant.merchant_accounts
    USING (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.merchant_account_id', true), '')::uuid);

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
