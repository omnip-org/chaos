CREATE TYPE integration.payment_provider AS ENUM ('stripe');
CREATE TYPE integration.shipping_provider AS ENUM ('manual');

CREATE TABLE integration.payment_provider_accounts (
    id                           UUID                          NOT NULL PRIMARY KEY,
    store_id                     UUID                          NOT NULL,
    provider                     integration.payment_provider  NOT NULL,
    display_name                 TEXT                          NOT NULL DEFAULT 'Payment Provider',
    credential_secret_reference  TEXT,
    webhook_secret_reference     TEXT,
    meta                         JSONB,
    created_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT payment_provider_accounts_store_provider_key         UNIQUE (store_id, provider),
    CONSTRAINT payment_provider_accounts_store_id_fkey              FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT payment_provider_accounts_display_name_length_check  CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT payment_provider_accounts_credential_reference_check CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_webhook_reference_check    CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT payment_provider_accounts_meta_object_check          CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT payment_provider_accounts_meta_size_check            CHECK (meta IS NULL OR pg_column_size(meta) <= 32768)
);

CREATE TABLE integration.shipping_provider_accounts (
    id                           UUID                          NOT NULL PRIMARY KEY,
    store_id                     UUID                          NOT NULL,
    provider                     integration.shipping_provider NOT NULL,
    display_name                 TEXT                          NOT NULL DEFAULT 'Shipping Provider',
    credential_secret_reference  TEXT,
    webhook_secret_reference     TEXT,
    meta                         JSONB,
    created_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                   TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT shipping_provider_accounts_store_provider_key         UNIQUE (store_id, provider),
    CONSTRAINT shipping_provider_accounts_store_id_fkey              FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT shipping_provider_accounts_display_name_length_check  CHECK (length(trim(display_name)) BETWEEN 1 AND 120),
    CONSTRAINT shipping_provider_accounts_credential_reference_check CHECK (credential_secret_reference IS NULL OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(credential_secret_reference) <= 32768 AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT shipping_provider_accounts_webhook_reference_check    CHECK (webhook_secret_reference IS NULL OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$' OR (char_length(webhook_secret_reference) <= 32768 AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$')),
    CONSTRAINT shipping_provider_accounts_meta_object_check          CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT shipping_provider_accounts_meta_size_check            CHECK (meta IS NULL OR pg_column_size(meta) <= 32768)
);

CREATE INDEX payment_provider_accounts_store_created_idx ON integration.payment_provider_accounts (store_id, created_at DESC, id DESC);
CREATE INDEX shipping_provider_accounts_store_created_idx ON integration.shipping_provider_accounts (store_id, created_at DESC, id DESC);

ALTER TABLE integration.payment_provider_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration.shipping_provider_accounts ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON integration.payment_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON integration.shipping_provider_accounts
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON integration.payment_provider_accounts,
       integration.shipping_provider_accounts
    TO chaos_runtime;
