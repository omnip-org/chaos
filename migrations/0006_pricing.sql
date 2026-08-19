CREATE TYPE pricing.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE pricing.tax_rule_status AS ENUM ('active', 'archived');

CREATE TYPE pricing.promotion_status AS ENUM ('active', 'archived');

CREATE TYPE pricing.promotion_trigger AS ENUM ('automatic', 'code');

CREATE TYPE pricing.promotion_value_kind AS ENUM ('percentage', 'fixed_amount');

CREATE TABLE pricing.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    code                 extensions.citext            NOT NULL,
    name                 TEXT                         NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    tax_inclusive        BOOLEAN                      NOT NULL DEFAULT false,
    status               pricing.price_list_status    NOT NULL DEFAULT 'draft',
    starts_at            TIMESTAMPTZ,
    ends_at              TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES merchant.store_currencies(store_id, currency),
    CONSTRAINT price_lists_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$'
    ),
    CONSTRAINT price_lists_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT price_lists_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT price_lists_validity_window_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR ends_at > starts_at
    )
);

CREATE TABLE pricing.prices (
    id                   UUID         NOT NULL PRIMARY KEY,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, price_list_id, product_variant_id),
    FOREIGN KEY (store_id, price_list_id)
        REFERENCES pricing.price_lists(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT prices_amount_nonnegative_check CHECK (
        amount_minor >= 0
    )
);

CREATE TABLE pricing.tax_rules (
    id                    UUID                    NOT NULL PRIMARY KEY,
    store_id              UUID                    NOT NULL,
    code                  TEXT                    NOT NULL,
    name                  TEXT                    NOT NULL,
    country_code          CHAR(2)                 NOT NULL,
    rate_basis_points     INTEGER                 NOT NULL,
    status                pricing.tax_rule_status NOT NULL DEFAULT 'active',
    created_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT tax_rules_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT tax_rules_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_rules_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT tax_rules_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE pricing.promotions (
    id                            UUID                         NOT NULL PRIMARY KEY,
    store_id                      UUID                         NOT NULL,
    handle                        TEXT                         NOT NULL,
    name                          TEXT                         NOT NULL,
    trigger                       pricing.promotion_trigger    NOT NULL,
    redemption_code               extensions.citext,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                      NOT NULL,
    minimum_subtotal_amount_minor BIGINT                       NOT NULL DEFAULT 0,
    priority                      SMALLINT                     NOT NULL DEFAULT 100,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    status                        pricing.promotion_status     NOT NULL DEFAULT 'active',
    created_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                    TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, handle),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, currency)
        REFERENCES merchant.store_currencies(store_id, currency),
    CONSTRAINT promotions_handle_format_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT promotions_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT promotions_redemption_shape_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code::text ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT promotions_value_shape_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT promotions_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT promotions_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT promotions_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT promotions_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    )
);

ALTER TABLE pricing.price_lists
    ADD UNIQUE (store_id, id, currency);

CREATE INDEX price_lists_store_activation_idx
    ON pricing.price_lists (store_id,
        status,
        currency,
        starts_at,
        ends_at
    );

CREATE INDEX prices_variant_lookup_idx
    ON pricing.prices (store_id,
        product_variant_id,
        price_list_id
    );

CREATE UNIQUE INDEX tax_rules_active_country_key
    ON pricing.tax_rules (store_id, country_code)
    WHERE status = 'active';

CREATE INDEX tax_rules_store_status_idx
    ON pricing.tax_rules (store_id, status, created_at, id);

CREATE UNIQUE INDEX promotions_active_redemption_code_key
    ON pricing.promotions (store_id, redemption_code)
    WHERE status = 'active' AND redemption_code IS NOT NULL;

CREATE INDEX promotions_checkout_lookup_idx
    ON pricing.promotions (store_id, currency, status, trigger, priority, id
    );

ALTER TABLE pricing.price_lists ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.prices ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.tax_rules ENABLE ROW LEVEL SECURITY;

ALTER TABLE pricing.promotions ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON pricing.price_lists
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.prices
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.tax_rules
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON pricing.promotions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA pricing TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA pricing TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA pricing
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
