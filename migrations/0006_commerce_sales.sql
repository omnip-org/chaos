CREATE TYPE commerce.cart_status AS ENUM ('active', 'completed', 'abandoned');

CREATE TYPE commerce.order_status AS ENUM ('pending', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');

CREATE TYPE commerce.order_payment_status AS ENUM (
    'pending',
    'paid',
    'failed',
    'partially_refunded',
    'refunded'
);

CREATE TYPE commerce.order_shipping_status AS ENUM (
    'pending',
    'shipped',
    'delivered',
    'cancelled'
);

CREATE TABLE commerce.shoppers (
    id             UUID        NOT NULL PRIMARY KEY,
    store_id       UUID        NOT NULL,
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE
);

CREATE TABLE commerce.carts (
    id                   UUID                NOT NULL PRIMARY KEY,
    store_id             UUID                NOT NULL,
    sales_channel_id     UUID                NOT NULL,
    shopper_id           UUID                NOT NULL,
    price_list_id        UUID                NOT NULL,
    currency             CHAR(3)             NOT NULL,
    locale               VARCHAR(63)         NOT NULL DEFAULT 'en-US',
    status               commerce.cart_status   NOT NULL DEFAULT 'active',
    version              BIGINT              NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id),
    FOREIGN KEY (store_id, shopper_id)
        REFERENCES commerce.shoppers(store_id, id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT carts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT carts_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT carts_version_nonnegative_check CHECK (version >= 0)
);

CREATE TABLE commerce.cart_lines (
    store_id                UUID        NOT NULL,
    cart_id                 UUID        NOT NULL,
    product_id              UUID        NOT NULL,
    product_variant_id      UUID        NOT NULL,
    product_title           TEXT        NOT NULL,
    variant_title           TEXT        NOT NULL,
    sku                     TEXT,
    requires_shipping       BOOLEAN     NOT NULL,
    track_inventory         BOOLEAN     NOT NULL,
    quantity                INTEGER     NOT NULL,
    unit_price_amount_minor BIGINT      NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, cart_id, product_variant_id),
    FOREIGN KEY (store_id, cart_id)
        REFERENCES commerce.carts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id),
    CONSTRAINT cart_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT cart_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT cart_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT cart_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT cart_lines_unit_price_nonnegative_check CHECK (unit_price_amount_minor >= 0)
);

CREATE TABLE commerce.orders (
    id                       UUID                               NOT NULL PRIMARY KEY,
    store_id                 UUID                               NOT NULL,
    order_number             TEXT                               NOT NULL,
    sales_channel_id         UUID                               NOT NULL,
    cart_id                  UUID                               NOT NULL,
    shopper_id               UUID                               NOT NULL,
    price_list_id            UUID                               NOT NULL,
    currency                 CHAR(3)                            NOT NULL,
    locale                   VARCHAR(63)                        NOT NULL DEFAULT 'en-US',
    status                   commerce.order_status              NOT NULL DEFAULT 'pending',
    payment_status           commerce.order_payment_status      NOT NULL DEFAULT 'pending',
    shipping_status          commerce.order_shipping_status     NOT NULL DEFAULT 'pending',
    stripe_checkout_session_id TEXT,
    stripe_payment_intent_id TEXT,
    stripe_charge_id         TEXT,
    stripe_refund_id         TEXT,
    payment_failure_code     TEXT,
    refunded_amount_minor    BIGINT                             NOT NULL DEFAULT 0,
    shipping_provider        TEXT,
    shipping_provider_reference TEXT,
    shipping_tracking_number TEXT,
    shipping_tracking_url    TEXT,
    subtotal_amount_minor    BIGINT                             NOT NULL,
    discount_amount_minor    BIGINT                             NOT NULL,
    tax_amount_minor         BIGINT                             NOT NULL,
    shipping_amount_minor    BIGINT                             NOT NULL,
    total_amount_minor       BIGINT                             NOT NULL,
    contact_email            extensions.citext,
    contact_phone            TEXT,
    billing_full_name        TEXT,
    billing_company          TEXT,
    billing_address_line1    TEXT,
    billing_address_line2    TEXT,
    billing_locality         TEXT,
    billing_administrative_area TEXT,
    billing_postal_code      TEXT,
    billing_country_code     CHAR(2),
    shipping_full_name       TEXT,
    shipping_company         TEXT,
    shipping_address_line1   TEXT,
    shipping_address_line2   TEXT,
    shipping_locality        TEXT,
    shipping_administrative_area TEXT,
    shipping_postal_code     TEXT,
    shipping_country_code    CHAR(2),
    created_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ                        NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, order_number),
    UNIQUE (store_id, id, shopper_id),
    FOREIGN KEY (store_id, cart_id)
        REFERENCES commerce.carts(store_id, id),
    FOREIGN KEY (store_id, shopper_id)
        REFERENCES commerce.shoppers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES commerce.price_lists(store_id, id, currency),
    CONSTRAINT orders_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_order_number_check CHECK (
        order_number ~ '^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$'
    ),
    CONSTRAINT orders_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT orders_amounts_check CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor >= 0
        AND refunded_amount_minor >= 0
        AND refunded_amount_minor <= total_amount_minor
    ),
    CONSTRAINT orders_contact_email_length_check CHECK (
        contact_email IS NULL OR length(trim(contact_email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT orders_contact_phone_format_check CHECK (
        contact_phone IS NULL OR contact_phone ~ '^\+[1-9][0-9]{7,14}$'
    ),
    CONSTRAINT orders_billing_country_code_check CHECK (
        billing_country_code IS NULL OR billing_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT orders_shipping_country_code_check CHECK (
        shipping_country_code IS NULL OR shipping_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT orders_stripe_checkout_session_check CHECK (
        stripe_checkout_session_id IS NULL
        OR length(trim(stripe_checkout_session_id)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_stripe_payment_intent_check CHECK (
        stripe_payment_intent_id IS NULL OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_stripe_charge_check CHECK (
        stripe_charge_id IS NULL OR stripe_charge_id ~ '^ch_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_stripe_refund_check CHECK (
        stripe_refund_id IS NULL OR stripe_refund_id ~ '^re_[A-Za-z0-9]+$'
    ),
    CONSTRAINT orders_payment_failure_code_check CHECK (
        payment_failure_code IS NULL OR length(trim(payment_failure_code)) BETWEEN 1 AND 2000
    ),
    CONSTRAINT orders_shipping_provider_check CHECK (
        shipping_provider IS NULL OR length(trim(shipping_provider)) BETWEEN 1 AND 64
    ),
    CONSTRAINT orders_shipping_reference_check CHECK (
        shipping_provider_reference IS NULL
        OR length(trim(shipping_provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_shipping_tracking_number_check CHECK (
        shipping_tracking_number IS NULL
        OR length(trim(shipping_tracking_number)) BETWEEN 1 AND 255
    ),
    CONSTRAINT orders_shipping_tracking_url_check CHECK (
        shipping_tracking_url IS NULL
        OR (length(shipping_tracking_url) BETWEEN 9 AND 2048 AND shipping_tracking_url ~ '^https://')
    )
);

CREATE TABLE commerce.order_tracking_tokens (
    store_id       UUID        NOT NULL,
    order_id       UUID        NOT NULL,
    token_digest   BYTEA       NOT NULL,
    expires_at     TIMESTAMPTZ NOT NULL,
    last_used_at   TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, order_id),
    UNIQUE (store_id, token_digest),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_tracking_tokens_digest_check CHECK (octet_length(token_digest) = 32),
    CONSTRAINT order_tracking_tokens_expiry_check CHECK (expires_at > created_at)
);

CREATE TABLE commerce.order_lines (
    store_id                 UUID        NOT NULL,
    order_id                 UUID        NOT NULL,
    position                 SMALLINT    NOT NULL,
    product_id               UUID        NOT NULL,
    product_variant_id       UUID        NOT NULL,
    product_title            TEXT        NOT NULL,
    variant_title            TEXT        NOT NULL,
    sku                      TEXT,
    requires_shipping        BOOLEAN     NOT NULL,
    track_inventory          BOOLEAN     NOT NULL,
    quantity                 INTEGER     NOT NULL,
    unit_price_amount_minor  BIGINT      NOT NULL,
    subtotal_amount_minor    BIGINT      NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, order_id, position),
    UNIQUE (store_id, order_id, product_variant_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    CONSTRAINT order_lines_position_check CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT order_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT order_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT order_lines_amounts_check CHECK (
        unit_price_amount_minor >= 0
        AND subtotal_amount_minor = unit_price_amount_minor * quantity
        AND subtotal_amount_minor >= 0
    )
);

CREATE TABLE commerce.order_transitions (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    order_id             UUID                         NOT NULL,
    from_status          commerce.order_status,
    to_status            commerce.order_status           NOT NULL,
    kind                 commerce.order_transition_kind NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                  NOT NULL,

    UNIQUE (store_id, order_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES commerce.orders(store_id, id),
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT order_transitions_shape_check CHECK (
        (kind = 'created' AND from_status IS NULL AND to_status = 'pending')
        OR (kind = 'confirmed' AND from_status = 'pending' AND to_status = 'confirmed')
        OR (kind = 'cancelled' AND from_status = 'pending' AND to_status = 'cancelled')
    )
);

ALTER TABLE commerce.orders
    ADD UNIQUE (store_id, id, currency);

CREATE TABLE commerce.payment_provider_accounts (
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    provider                   TEXT        NOT NULL,
    display_name               TEXT        NOT NULL DEFAULT 'Payment provider',
    credential_secret_reference TEXT,
    previous_credential_secret_reference TEXT,
    credential_rotation_expires_at TIMESTAMPTZ,
    webhook_secret_reference    TEXT,
    previous_webhook_secret_reference TEXT,
    webhook_rotation_expires_at TIMESTAMPTZ,
    readiness_status            TEXT        NOT NULL DEFAULT 'unchecked',
    readiness_snapshot          JSONB,
    readiness_checked_at        TIMESTAMPTZ,
    readiness_valid_until       TIMESTAMPTZ,
    readiness_reconcile_at      TIMESTAMPTZ,
    readiness_locked_by         UUID,
    readiness_locked_at         TIMESTAMPTZ,
    readiness_reconcile_attempts INTEGER     NOT NULL DEFAULT 0,
    readiness_last_error        TEXT,
    enabled                    BOOLEAN     NOT NULL DEFAULT false,
    created_by_user_id          UUID,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    CONSTRAINT payment_provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT payment_provider_accounts_provider_length_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT payment_provider_accounts_stripe_only_check CHECK (
        provider = 'stripe_checkout'
    ),
    CONSTRAINT payment_provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT payment_provider_accounts_credential_reference_check CHECK (
        credential_secret_reference IS NULL
        OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_webhook_reference_check CHECK (
        webhook_secret_reference IS NULL
        OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(webhook_secret_reference) <= 32768
            AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_previous_webhook_reference_check CHECK (
        previous_webhook_secret_reference IS NULL
        OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_webhook_secret_reference) <= 32768
            AND previous_webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT payment_provider_accounts_webhook_rotation_shape_check CHECK (
        (previous_webhook_secret_reference IS NULL AND webhook_rotation_expires_at IS NULL)
        OR (previous_webhook_secret_reference IS NOT NULL AND webhook_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_readiness_status_check CHECK (
        readiness_status IN ('unchecked', 'ready', 'action_required')
    ),
    CONSTRAINT payment_provider_accounts_readiness_shape_check CHECK (
        (
            readiness_status = 'unchecked'
            AND readiness_snapshot IS NULL
            AND readiness_checked_at IS NULL
            AND readiness_valid_until IS NULL
            AND readiness_reconcile_at IS NULL
        )
        OR (
            readiness_status <> 'unchecked'
            AND jsonb_typeof(readiness_snapshot) = 'object'
            AND pg_column_size(readiness_snapshot) <= 8192
            AND readiness_checked_at IS NOT NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_enabled_readiness_check CHECK (
        NOT enabled
        OR (
            readiness_status = 'ready'
            AND readiness_valid_until IS NOT NULL
            AND readiness_reconcile_at IS NOT NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_readiness_validity_check CHECK (
        (
            readiness_status = 'ready'
            AND readiness_valid_until > readiness_checked_at
            AND readiness_reconcile_at IS NOT NULL
        )
        OR (
            readiness_status <> 'ready'
            AND readiness_valid_until IS NULL
            AND readiness_reconcile_at IS NULL
        )
    ),
    CONSTRAINT payment_provider_accounts_readiness_lock_shape_check CHECK (
        (readiness_locked_by IS NULL AND readiness_locked_at IS NULL)
        OR (readiness_locked_by IS NOT NULL AND readiness_locked_at IS NOT NULL)
    ),
    CONSTRAINT payment_provider_accounts_readiness_attempts_check CHECK (
        readiness_reconcile_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT payment_provider_accounts_readiness_error_length_check CHECK (
        readiness_last_error IS NULL OR length(readiness_last_error) BETWEEN 1 AND 2000
    )
);

CREATE INDEX shoppers_store_seen_idx
    ON commerce.shoppers (store_id, last_seen_at DESC, id DESC);

CREATE INDEX carts_channel_updated_idx
    ON commerce.carts (store_id,
        sales_channel_id,
        status,
        updated_at DESC,
        id DESC
    );

CREATE INDEX cart_lines_variant_lookup_idx
    ON commerce.cart_lines (store_id, product_variant_id, cart_id);

CREATE INDEX orders_channel_created_idx
    ON commerce.orders (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX order_tracking_tokens_expiry_idx
    ON commerce.order_tracking_tokens (expires_at, store_id, order_id);

CREATE INDEX order_transitions_order_time_idx
    ON commerce.order_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

CREATE INDEX payment_provider_accounts_store_created_idx
    ON commerce.payment_provider_accounts (store_id, created_at DESC, id DESC);

CREATE INDEX payment_provider_accounts_readiness_due_idx
    ON commerce.payment_provider_accounts (readiness_reconcile_at, id)
    WHERE enabled;

CREATE FUNCTION commerce.resolve_provider_account(
    requested_provider             TEXT,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id            UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, account.store_id
      FROM commerce.payment_provider_accounts AS account
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled;
$$;

CREATE FUNCTION commerce.resolve_provider_webhook_secret_references(
    requested_provider             TEXT,
    requested_provider_account_id  UUID
)
RETURNS TABLE (
    provider_account_id UUID,
    secret_reference    TEXT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT account.id, candidate.secret_reference
      FROM commerce.payment_provider_accounts AS account
      CROSS JOIN LATERAL (
          VALUES
              (account.webhook_secret_reference, 0),
              (
                  CASE WHEN account.webhook_rotation_expires_at > CURRENT_TIMESTAMP
                       THEN account.previous_webhook_secret_reference END,
                  1
              )
      ) AS candidate(secret_reference, priority)
     WHERE account.provider = requested_provider
       AND account.id = requested_provider_account_id
       AND account.enabled
       AND candidate.secret_reference IS NOT NULL
     ORDER BY candidate.priority;
$$;

CREATE FUNCTION commerce.claim_provider_readiness_checks(
    worker_id   UUID,
    batch_size  INTEGER,
    claimed_at  TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id       UUID,
    store_id                  UUID,
    provider                  TEXT,
    credential_secret_reference TEXT,
    attempts                  INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE commerce.payment_provider_accounts AS account
           SET enabled = false,
               readiness_status = 'action_required',
               readiness_snapshot = jsonb_set(
                   jsonb_set(account.readiness_snapshot, '{ready}', 'false'::jsonb, true),
                   '{blocker_codes}', '["readiness_expired"]'::jsonb, true
               ),
               readiness_valid_until = NULL,
               readiness_reconcile_at = NULL,
               readiness_locked_by = NULL,
               readiness_locked_at = NULL,
               readiness_last_error = NULL,
               updated_at = claimed_at
         WHERE account.enabled
           AND account.readiness_valid_until <= claimed_at
        RETURNING account.id
    ), claimable AS (
        SELECT account.id
          FROM commerce.payment_provider_accounts AS account
         WHERE account.enabled
           AND account.credential_secret_reference IS NOT NULL
           AND account.readiness_valid_until > claimed_at
           AND account.readiness_reconcile_at <= claimed_at
           AND (
               account.readiness_locked_at IS NULL
               OR account.readiness_locked_at <= stale_before
           )
         ORDER BY account.readiness_reconcile_at, account.id
         FOR UPDATE SKIP LOCKED
         LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE commerce.payment_provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

CREATE FUNCTION commerce.finish_provider_readiness_check(
    requested_provider_account_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    ready BOOLEAN,
    requested_readiness_snapshot JSONB,
    observed_at TIMESTAMPTZ,
    failure_message TEXT
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE commerce.payment_provider_accounts AS account
       SET enabled = CASE
               WHEN succeeded THEN account.enabled AND ready
               ELSE account.enabled
           END,
           readiness_status = CASE
               WHEN succeeded AND ready THEN 'ready'
               WHEN succeeded THEN 'action_required'
               ELSE account.readiness_status
           END,
           readiness_snapshot = CASE
               WHEN succeeded THEN requested_readiness_snapshot
               ELSE account.readiness_snapshot
           END,
           readiness_checked_at = CASE
               WHEN succeeded THEN observed_at
               ELSE account.readiness_checked_at
           END,
           readiness_valid_until = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '24 hours'
               WHEN succeeded THEN NULL
               ELSE account.readiness_valid_until
           END,
           readiness_reconcile_at = CASE
               WHEN succeeded AND ready THEN observed_at + INTERVAL '6 hours'
               WHEN succeeded THEN NULL
               ELSE observed_at + make_interval(
                   secs => least(power(2, greatest(account.readiness_reconcile_attempts - 1, 0))::integer, 3600)
               )
           END,
           readiness_locked_by = NULL,
           readiness_locked_at = NULL,
           readiness_reconcile_attempts = CASE
               WHEN succeeded THEN 0
               ELSE account.readiness_reconcile_attempts
           END,
           readiness_last_error = CASE
               WHEN succeeded THEN NULL
               ELSE COALESCE(NULLIF(left(failure_message, 2000), ''), 'readiness check failed')
           END,
           updated_at = CASE WHEN succeeded THEN observed_at ELSE account.updated_at END
     WHERE account.id = requested_provider_account_id
       AND account.readiness_locked_by = worker_id
    RETURNING true;
$$;

ALTER TABLE commerce.shoppers ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.shoppers FORCE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.shoppers
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

ALTER TABLE commerce.carts ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.cart_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.orders ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_tracking_tokens ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.order_transitions ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.carts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.cart_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.orders
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_tracking_tokens
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.order_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.order_transitions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

REVOKE ALL ON FUNCTION commerce.authenticate_publishable_key(TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.rebuild_store_products(UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.process_events(INTEGER, TIMESTAMPTZ) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION commerce.resolve_provider_account(TEXT, UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.authenticate_publishable_key(TEXT) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.rebuild_store_products(UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.process_events(INTEGER, TIMESTAMPTZ) TO chaos_runtime;

GRANT EXECUTE
    ON FUNCTION commerce.resolve_provider_webhook_secret_references(TEXT, UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION commerce.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
)
    TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA commerce TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA commerce TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA commerce
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE DELETE ON commerce.orders FROM chaos_runtime;

REVOKE DELETE ON commerce.collections FROM chaos_runtime;

REVOKE DELETE ON commerce.media_assets FROM chaos_runtime;

REVOKE DELETE ON commerce.reviews FROM chaos_runtime;

GRANT USAGE ON SCHEMA commerce TO chaos_runtime;
