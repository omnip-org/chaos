CREATE TYPE sales.cart_status AS ENUM ('active', 'completed', 'abandoned');

CREATE TYPE sales.checkout_status AS ENUM ('pending', 'completed', 'expired');

CREATE TYPE sales.address_kind AS ENUM ('billing', 'shipping');

CREATE TYPE sales.order_status AS ENUM ('pending', 'confirmed', 'cancelled');

CREATE TYPE sales.order_transition_kind AS ENUM ('created', 'confirmed', 'cancelled');

CREATE TYPE sales.order_fulfillment_status AS ENUM (
    'unfulfilled',
    'partially_fulfilled',
    'fulfilled'
);

CREATE TYPE sales.order_delivery_status AS ENUM (
    'not_delivered',
    'partially_delivered',
    'delivered'
);

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TYPE fulfillment.shipping_service_status AS ENUM ('active', 'archived');

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TYPE payments.payment_attempt_status AS ENUM (
    'pending',
    'authorized',
    'captured',
    'failed',
    'cancelled'
);

CREATE TYPE payments.refund_status AS ENUM ('pending', 'succeeded', 'failed');

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TYPE fulfillment.fulfillment_status AS ENUM (
    'pending',
    'shipped',
    'delivered',
    'cancelled'
);

CREATE TYPE fulfillment.return_status AS ENUM (
    'requested',
    'authorized',
    'received',
    'completed',
    'rejected'
);

CREATE TYPE fulfillment.return_disposition AS ENUM ('restock', 'discard');

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TABLE sales.customers (
    id                  UUID              NOT NULL PRIMARY KEY,
    store_id            UUID              NOT NULL,
    user_id             UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,
    created_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TIMESTAMPTZ       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, user_id),
    UNIQUE (store_id, email),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES identity.users(id),
    CONSTRAINT customers_email_length_check CHECK (length(trim(email::text)) BETWEEN 3 AND 320),
    CONSTRAINT customers_phone_format_check CHECK (phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$')
);

CREATE TABLE sales.customer_addresses (
    id                   UUID     NOT NULL PRIMARY KEY,
    store_id             UUID     NOT NULL,
    customer_id          UUID     NOT NULL,
    label                TEXT     NOT NULL,
    full_name            TEXT     NOT NULL,
    company              TEXT,
    address_line1        TEXT     NOT NULL,
    address_line2        TEXT,
    locality             TEXT     NOT NULL,
    administrative_area  TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)  NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, customer_id, id),
    CONSTRAINT customer_addresses_customer_label_key
        UNIQUE (store_id, customer_id, label),
    CONSTRAINT customer_addresses_customer_fkey
        FOREIGN KEY (store_id, customer_id)
        REFERENCES sales.customers(store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_addresses_label_length_check CHECK (length(trim(label)) BETWEEN 1 AND 64),
    CONSTRAINT customer_addresses_full_name_length_check CHECK (length(trim(full_name)) BETWEEN 1 AND 200),
    CONSTRAINT customer_addresses_company_length_check CHECK (company IS NULL OR length(trim(company)) BETWEEN 1 AND 200),
    CONSTRAINT customer_addresses_line1_length_check CHECK (length(trim(address_line1)) BETWEEN 1 AND 255),
    CONSTRAINT customer_addresses_line2_length_check CHECK (address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255),
    CONSTRAINT customer_addresses_locality_length_check CHECK (length(trim(locality)) BETWEEN 1 AND 100),
    CONSTRAINT customer_addresses_area_length_check CHECK (administrative_area IS NULL OR length(trim(administrative_area)) BETWEEN 1 AND 100),
    CONSTRAINT customer_addresses_postal_code_length_check CHECK (postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32),
    CONSTRAINT customer_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE sales.customer_shopper_links (
    store_id            UUID        NOT NULL,
    customer_id         UUID        NOT NULL,
    shopper_id          UUID        NOT NULL,
    sales_channel_id    UUID        NOT NULL,
    linked_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, shopper_id),
    CONSTRAINT customer_shopper_links_customer_fkey
        FOREIGN KEY (store_id, customer_id)
        REFERENCES sales.customers(store_id, id) ON DELETE CASCADE,
    CONSTRAINT customer_shopper_links_channel_fkey
        FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE sales.carts (
    id                   UUID                NOT NULL PRIMARY KEY,
    store_id             UUID                NOT NULL,
    sales_channel_id     UUID                NOT NULL,
    shopper_id           UUID                NOT NULL,
    customer_id          UUID,
    price_list_id        UUID                NOT NULL,
    currency             CHAR(3)             NOT NULL,
    locale               VARCHAR(63)         NOT NULL DEFAULT 'en-US',
    status               sales.cart_status   NOT NULL DEFAULT 'active',
    version              BIGINT              NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES sales.customers(store_id, id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(store_id, id, currency),
    CONSTRAINT carts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT carts_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT carts_version_nonnegative_check CHECK (version >= 0)
);

CREATE TABLE sales.cart_lines (
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
    tax_inclusive           BOOLEAN     NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, cart_id, product_variant_id),
    FOREIGN KEY (store_id, cart_id)
        REFERENCES sales.carts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, product_id, id),
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

CREATE TABLE sales.checkouts (
    id                     UUID                    NOT NULL PRIMARY KEY,
    store_id               UUID                    NOT NULL,
    cart_id                UUID                    NOT NULL,
    shopper_id             UUID                    NOT NULL,
    customer_id            UUID,
    sales_channel_id       UUID                    NOT NULL,
    price_list_id          UUID                    NOT NULL,
    inventory_reservation_id UUID,
    currency               CHAR(3)                 NOT NULL,
    locale                 VARCHAR(63)             NOT NULL DEFAULT 'en-US',
    status                 sales.checkout_status   NOT NULL DEFAULT 'pending',
    subtotal_amount_minor  BIGINT                  NOT NULL,
    discount_amount_minor  BIGINT                  NOT NULL,
    tax_amount_minor       BIGINT                  NOT NULL,
    tax_inclusive          BOOLEAN                 NOT NULL,
    shipping_amount_minor  BIGINT                  NOT NULL,
    total_amount_minor     BIGINT                  NOT NULL,
    expires_at             TIMESTAMPTZ             NOT NULL,
    closed_at              TIMESTAMPTZ,
    expiry_locked_by       UUID,
    expiry_locked_at       TIMESTAMPTZ,
    created_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, cart_id),
    UNIQUE (store_id, inventory_reservation_id),
    FOREIGN KEY (store_id, cart_id, shopper_id)
        REFERENCES sales.carts(store_id, id, shopper_id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES sales.customers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(store_id, id, currency),
    FOREIGN KEY (store_id, inventory_reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id),
    CONSTRAINT checkouts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT checkouts_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT checkouts_amounts_check CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
            + shipping_amount_minor
    ),
    CONSTRAINT checkouts_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT checkouts_closure_check CHECK (
        (status = 'pending' AND closed_at IS NULL)
        OR (status <> 'pending' AND closed_at IS NOT NULL)
    ),
    CONSTRAINT checkouts_expiry_lease_shape_check CHECK (
        (expiry_locked_by IS NULL) = (expiry_locked_at IS NULL)
        AND (status = 'pending' OR expiry_locked_by IS NULL)
    )
);

CREATE TABLE sales.checkout_contacts (
    store_id            UUID              NOT NULL,
    checkout_id         UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id) ON DELETE CASCADE,
    CONSTRAINT checkout_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT checkout_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE TABLE sales.checkout_addresses (
    store_id             UUID               NOT NULL,
    checkout_id          UUID               NOT NULL,
    kind                 sales.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (store_id, checkout_id, kind),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id) ON DELETE CASCADE,
    CONSTRAINT checkout_addresses_full_name_length_check CHECK (
        length(trim(full_name)) BETWEEN 1 AND 200
    ),
    CONSTRAINT checkout_addresses_company_length_check CHECK (
        company IS NULL OR length(trim(company)) BETWEEN 1 AND 200
    ),
    CONSTRAINT checkout_addresses_line1_length_check CHECK (
        length(trim(address_line1)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_addresses_line2_length_check CHECK (
        address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_addresses_locality_length_check CHECK (
        length(trim(locality)) BETWEEN 1 AND 100
    ),
    CONSTRAINT checkout_addresses_area_length_check CHECK (
        administrative_area IS NULL
        OR length(trim(administrative_area)) BETWEEN 1 AND 100
    ),
    CONSTRAINT checkout_addresses_postal_code_length_check CHECK (
        postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT checkout_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE sales.checkout_tax_calculations (
    store_id            UUID    NOT NULL,
    checkout_id         UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, tax_rule_id)
        REFERENCES pricing.tax_rules(store_id, id),
    CONSTRAINT checkout_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT checkout_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE sales.checkout_promotion_calculations (
    store_id                      UUID                      NOT NULL,
    checkout_id                   UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       pricing.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, promotion_id)
        REFERENCES pricing.promotions(store_id, id),
    CONSTRAINT checkout_promotion_handle_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT checkout_promotion_name_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_promotion_trigger_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT checkout_promotion_value_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT checkout_promotion_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT checkout_promotion_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT checkout_promotion_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    ),
    CONSTRAINT checkout_promotion_discount_check CHECK (discount_amount_minor > 0),
    CONSTRAINT checkout_promotion_currency_check CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE sales.checkout_lines (
    store_id                 UUID        NOT NULL,
    checkout_id              UUID        NOT NULL,
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
    discount_amount_minor    BIGINT      NOT NULL,
    tax_amount_minor         BIGINT      NOT NULL,
    total_amount_minor       BIGINT      NOT NULL,
    tax_inclusive            BOOLEAN     NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, checkout_id, position),
    UNIQUE (store_id, checkout_id, product_variant_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id),
    CONSTRAINT checkout_lines_position_check CHECK (position BETWEEN 0 AND 998),
    CONSTRAINT checkout_lines_product_title_length_check CHECK (
        length(trim(product_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_lines_variant_title_length_check CHECK (
        length(trim(variant_title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT checkout_lines_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku)) BETWEEN 1 AND 64
    ),
    CONSTRAINT checkout_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT checkout_lines_amounts_check CHECK (
        unit_price_amount_minor >= 0
        AND subtotal_amount_minor = unit_price_amount_minor * quantity
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
        AND (NOT tax_inclusive OR tax_amount_minor <= subtotal_amount_minor - discount_amount_minor)
    )
);

CREATE TABLE sales.orders (
    id                       UUID                  NOT NULL PRIMARY KEY,
    store_id                 UUID                  NOT NULL,
    sales_channel_id         UUID                  NOT NULL,
    checkout_id              UUID                  NOT NULL,
    shopper_id               UUID                  NOT NULL,
    customer_id              UUID,
    inventory_reservation_id UUID,
    price_list_id            UUID                  NOT NULL,
    currency                 CHAR(3)               NOT NULL,
    locale                   VARCHAR(63)           NOT NULL DEFAULT 'en-US',
    status                   sales.order_status    NOT NULL DEFAULT 'pending',
    fulfillment_status       sales.order_fulfillment_status NOT NULL DEFAULT 'unfulfilled',
    delivery_status          sales.order_delivery_status NOT NULL DEFAULT 'not_delivered',
    subtotal_amount_minor    BIGINT                NOT NULL,
    discount_amount_minor    BIGINT                NOT NULL,
    tax_amount_minor         BIGINT                NOT NULL,
    tax_inclusive            BOOLEAN               NOT NULL,
    shipping_amount_minor    BIGINT                NOT NULL,
    total_amount_minor       BIGINT                NOT NULL,
    created_at               TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at               TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id, shopper_id)
        REFERENCES sales.checkouts(store_id, id, shopper_id),
    FOREIGN KEY (store_id, customer_id)
        REFERENCES sales.customers(store_id, id),
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    FOREIGN KEY (store_id, price_list_id, currency)
        REFERENCES pricing.price_lists(store_id, id, currency),
    FOREIGN KEY (store_id, inventory_reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id),
    CONSTRAINT orders_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT orders_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    ),
    CONSTRAINT orders_amounts_check CHECK (
        subtotal_amount_minor >= 0
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND shipping_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
            + shipping_amount_minor
    )
);

CREATE TABLE sales.order_contacts (
    store_id            UUID              NOT NULL,
    order_id            UUID              NOT NULL,
    email               extensions.citext NOT NULL,
    phone               TEXT,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_contacts_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    ),
    CONSTRAINT order_contacts_phone_format_check CHECK (
        phone IS NULL OR phone ~ '^\+[1-9][0-9]{7,14}$'
    )
);

CREATE TABLE sales.order_addresses (
    store_id             UUID               NOT NULL,
    order_id             UUID               NOT NULL,
    kind                 sales.address_kind NOT NULL,
    full_name            TEXT               NOT NULL,
    company              TEXT,
    address_line1        TEXT               NOT NULL,
    address_line2        TEXT,
    locality             TEXT               NOT NULL,
    administrative_area TEXT,
    postal_code          TEXT,
    country_code         CHAR(2)            NOT NULL,

    PRIMARY KEY (store_id, order_id, kind),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id) ON DELETE CASCADE,
    CONSTRAINT order_addresses_full_name_length_check CHECK (
        length(trim(full_name)) BETWEEN 1 AND 200
    ),
    CONSTRAINT order_addresses_company_length_check CHECK (
        company IS NULL OR length(trim(company)) BETWEEN 1 AND 200
    ),
    CONSTRAINT order_addresses_line1_length_check CHECK (
        length(trim(address_line1)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_addresses_line2_length_check CHECK (
        address_line2 IS NULL OR length(trim(address_line2)) BETWEEN 1 AND 255
    ),
    CONSTRAINT order_addresses_locality_length_check CHECK (
        length(trim(locality)) BETWEEN 1 AND 100
    ),
    CONSTRAINT order_addresses_area_length_check CHECK (
        administrative_area IS NULL
        OR length(trim(administrative_area)) BETWEEN 1 AND 100
    ),
    CONSTRAINT order_addresses_postal_code_length_check CHECK (
        postal_code IS NULL OR length(trim(postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT order_addresses_country_code_check CHECK (country_code ~ '^[A-Z]{2}$')
);

CREATE TABLE sales.order_tax_calculations (
    store_id            UUID    NOT NULL,
    order_id            UUID    NOT NULL,
    tax_rule_id         UUID    NOT NULL,
    rule_code           TEXT    NOT NULL,
    rule_name           TEXT    NOT NULL,
    country_code        CHAR(2) NOT NULL,
    rate_basis_points   INTEGER NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, tax_rule_id)
        REFERENCES pricing.tax_rules(store_id, id),
    CONSTRAINT order_tax_rule_code_length_check CHECK (length(trim(rule_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_tax_rule_name_length_check CHECK (length(trim(rule_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_tax_country_code_check CHECK (country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT order_tax_rate_range_check CHECK (rate_basis_points BETWEEN 0 AND 10000)
);

CREATE TABLE sales.order_promotion_calculations (
    store_id                      UUID                      NOT NULL,
    order_id                      UUID                      NOT NULL,
    promotion_id                  UUID                      NOT NULL,
    handle                        TEXT                      NOT NULL,
    name                          TEXT                      NOT NULL,
    trigger                       pricing.promotion_trigger NOT NULL,
    redemption_code               TEXT,
    value_kind                    pricing.promotion_value_kind NOT NULL,
    rate_basis_points             INTEGER,
    amount_minor                  BIGINT,
    maximum_amount_minor          BIGINT,
    currency                      CHAR(3)                   NOT NULL,
    minimum_subtotal_amount_minor BIGINT                    NOT NULL,
    priority                      SMALLINT                  NOT NULL,
    starts_at                     TIMESTAMPTZ,
    ends_at                       TIMESTAMPTZ,
    discount_amount_minor         BIGINT                    NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, promotion_id)
        REFERENCES pricing.promotions(store_id, id),
    CONSTRAINT order_promotion_handle_check CHECK (handle ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT order_promotion_name_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT order_promotion_trigger_check CHECK (
        (trigger = 'automatic' AND redemption_code IS NULL)
        OR (trigger = 'code' AND redemption_code ~ '^[A-Z0-9-]{1,64}$')
    ),
    CONSTRAINT order_promotion_value_check CHECK (
        (value_kind = 'percentage' AND rate_basis_points BETWEEN 1 AND 10000
            AND amount_minor IS NULL
            AND (maximum_amount_minor IS NULL OR maximum_amount_minor > 0))
        OR (value_kind = 'fixed_amount' AND rate_basis_points IS NULL
            AND amount_minor > 0 AND maximum_amount_minor IS NULL)
    ),
    CONSTRAINT order_promotion_minimum_check CHECK (minimum_subtotal_amount_minor >= 0),
    CONSTRAINT order_promotion_priority_check CHECK (priority BETWEEN 0 AND 32767),
    CONSTRAINT order_promotion_schedule_check CHECK (
        starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at
    ),
    CONSTRAINT order_promotion_discount_check CHECK (discount_amount_minor > 0),
    CONSTRAINT order_promotion_currency_check CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE sales.order_lines (
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
    discount_amount_minor    BIGINT      NOT NULL,
    tax_amount_minor         BIGINT      NOT NULL,
    total_amount_minor       BIGINT      NOT NULL,
    tax_inclusive            BOOLEAN     NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, order_id, position),
    UNIQUE (store_id, order_id, product_variant_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id),
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
        AND discount_amount_minor >= 0
        AND discount_amount_minor <= subtotal_amount_minor
        AND tax_amount_minor >= 0
        AND total_amount_minor = subtotal_amount_minor - discount_amount_minor
            + CASE WHEN tax_inclusive THEN 0 ELSE tax_amount_minor END
        AND (NOT tax_inclusive OR tax_amount_minor <= subtotal_amount_minor - discount_amount_minor)
    )
);

CREATE TABLE sales.order_transitions (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    order_id             UUID                         NOT NULL,
    from_status          sales.order_status,
    to_status            sales.order_status           NOT NULL,
    kind                 sales.order_transition_kind NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                  NOT NULL,

    UNIQUE (store_id, order_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id),
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT order_transitions_shape_check CHECK (
        (kind = 'created' AND from_status IS NULL AND to_status = 'pending')
        OR (kind = 'confirmed' AND from_status = 'pending' AND to_status = 'confirmed')
        OR (kind = 'cancelled' AND from_status = 'pending' AND to_status = 'cancelled')
    )
);

ALTER TABLE sales.orders
    ADD UNIQUE (store_id, id, currency);

CREATE TABLE sales.order_fulfillment_transitions (
    id                       UUID                           NOT NULL PRIMARY KEY,
    store_id                 UUID                           NOT NULL,
    order_id                 UUID                           NOT NULL,
    source_event_id          UUID                           NOT NULL UNIQUE,
    from_fulfillment_status  sales.order_fulfillment_status NOT NULL,
    to_fulfillment_status    sales.order_fulfillment_status NOT NULL,
    from_delivery_status     sales.order_delivery_status    NOT NULL,
    to_delivery_status       sales.order_delivery_status    NOT NULL,
    occurred_at              TIMESTAMPTZ                    NOT NULL,

    UNIQUE (store_id, order_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id),
    FOREIGN KEY (source_event_id) REFERENCES integration.outbox_events(id)
);

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TABLE fulfillment.shipping_services (
    id                         UUID                                NOT NULL PRIMARY KEY,
    store_id                   UUID                                NOT NULL,
    code                       TEXT                                NOT NULL,
    name                       TEXT                                NOT NULL,
    amount_minor               BIGINT                              NOT NULL,
    currency                   CHAR(3)                             NOT NULL,
    estimated_min_days         SMALLINT                            NOT NULL,
    estimated_max_days         SMALLINT                            NOT NULL,
    status                     fulfillment.shipping_service_status NOT NULL DEFAULT 'active',
    created_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                 TIMESTAMPTZ                         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id),
    FOREIGN KEY (store_id, currency)
        REFERENCES merchant.store_currencies(store_id, currency),
    CONSTRAINT shipping_services_code_format_check CHECK (code ~ '^[a-z0-9-]{1,64}$'),
    CONSTRAINT shipping_services_name_length_check CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT shipping_services_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT shipping_services_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT shipping_services_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE fulfillment.shipping_provider_accounts (
    id                                   UUID        NOT NULL PRIMARY KEY,
    store_id                             UUID        NOT NULL,
    provider                             TEXT        NOT NULL,
    display_name                         TEXT        NOT NULL,
    credential_secret_reference          TEXT        NOT NULL,
    previous_credential_secret_reference TEXT,
    credential_rotation_expires_at       TIMESTAMPTZ,
    origin_name                          TEXT        NOT NULL,
    origin_company                       TEXT,
    origin_address_line_1                TEXT        NOT NULL,
    origin_address_line_2                TEXT,
    origin_city                          TEXT        NOT NULL,
    origin_region                        TEXT,
    origin_postal_code                   TEXT        NOT NULL,
    origin_country_code                  CHAR(2)     NOT NULL,
    origin_phone                         TEXT,
    origin_email                         TEXT,
    enabled                              BOOLEAN     NOT NULL DEFAULT false,
    created_by_user_id                   UUID,
    created_at                           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    CONSTRAINT shipping_provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT shipping_provider_accounts_provider_format_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT shipping_provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_credential_reference_check CHECK (
        credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT shipping_provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{7,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT shipping_provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT shipping_provider_accounts_origin_name_length_check CHECK (
        length(trim(origin_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_company_length_check CHECK (
        origin_company IS NULL OR length(trim(origin_company)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_address_length_check CHECK (
        length(trim(origin_address_line_1)) BETWEEN 1 AND 200
        AND (origin_address_line_2 IS NULL OR length(trim(origin_address_line_2)) BETWEEN 1 AND 200)
    ),
    CONSTRAINT shipping_provider_accounts_origin_city_length_check CHECK (
        length(trim(origin_city)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_region_length_check CHECK (
        origin_region IS NULL OR length(trim(origin_region)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_provider_accounts_origin_postal_length_check CHECK (
        length(trim(origin_postal_code)) BETWEEN 1 AND 32
    ),
    CONSTRAINT shipping_provider_accounts_origin_country_check CHECK (
        origin_country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT shipping_provider_accounts_origin_phone_length_check CHECK (
        origin_phone IS NULL OR length(trim(origin_phone)) BETWEEN 1 AND 32
    ),
    CONSTRAINT shipping_provider_accounts_origin_email_length_check CHECK (
        origin_email IS NULL OR length(trim(origin_email)) BETWEEN 3 AND 254
    )
);

CREATE TABLE fulfillment.shipping_service_regions (
    store_id            UUID    NOT NULL,
    shipping_service_id UUID    NOT NULL,
    country_code        CHAR(2) NOT NULL,

    PRIMARY KEY (store_id, shipping_service_id, country_code),
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(store_id, id),
    CONSTRAINT shipping_service_regions_country_check CHECK (country_code ~ '^[A-Z]{2}$')
);

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TABLE sales.checkout_shipping_selections (
    store_id            UUID        NOT NULL,
    checkout_id         UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (store_id, checkout_id),
    FOREIGN KEY (store_id, checkout_id)
        REFERENCES sales.checkouts(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(store_id, id),
    CONSTRAINT checkout_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT checkout_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT checkout_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT checkout_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT checkout_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

CREATE TABLE sales.order_shipping_selections (
    store_id            UUID        NOT NULL,
    order_id            UUID        NOT NULL,
    shipping_service_id UUID        NOT NULL,
    service_code        TEXT        NOT NULL,
    service_name        TEXT        NOT NULL,
    amount_minor        BIGINT      NOT NULL,
    currency            CHAR(3)     NOT NULL,
    estimated_min_days  SMALLINT    NOT NULL,
    estimated_max_days  SMALLINT    NOT NULL,

    PRIMARY KEY (store_id, order_id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, shipping_service_id)
        REFERENCES fulfillment.shipping_services(store_id, id),
    CONSTRAINT order_shipping_code_length_check CHECK (length(trim(service_code)) BETWEEN 1 AND 64),
    CONSTRAINT order_shipping_name_length_check CHECK (length(trim(service_name)) BETWEEN 1 AND 120),
    CONSTRAINT order_shipping_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT order_shipping_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT order_shipping_estimate_check CHECK (
        estimated_min_days BETWEEN 0 AND 365
        AND estimated_max_days BETWEEN estimated_min_days AND 365
    )
);

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TABLE payments.provider_accounts (
    id                         UUID        NOT NULL PRIMARY KEY,
    store_id                   UUID        NOT NULL,
    provider                   TEXT        NOT NULL,
    display_name               TEXT        NOT NULL DEFAULT 'Payment provider',
    external_account_reference TEXT        NOT NULL,
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
    CONSTRAINT provider_accounts_store_provider_key
        UNIQUE (store_id, provider),
    UNIQUE (provider, external_account_reference),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id),
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id) ON DELETE SET NULL,
    CONSTRAINT provider_accounts_provider_length_check CHECK (
        provider ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT provider_accounts_display_name_length_check CHECK (
        length(trim(display_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT provider_accounts_external_reference_length_check CHECK (
        length(trim(external_account_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT provider_accounts_credential_reference_check CHECK (
        credential_secret_reference IS NULL
        OR credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(credential_secret_reference) <= 32768
            AND credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_previous_credential_reference_check CHECK (
        previous_credential_secret_reference IS NULL
        OR previous_credential_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_credential_secret_reference) <= 32768
            AND previous_credential_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_credential_rotation_shape_check CHECK (
        (previous_credential_secret_reference IS NULL AND credential_rotation_expires_at IS NULL)
        OR (previous_credential_secret_reference IS NOT NULL AND credential_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_webhook_reference_check CHECK (
        webhook_secret_reference IS NULL
        OR webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(webhook_secret_reference) <= 32768
            AND webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_previous_webhook_reference_check CHECK (
        previous_webhook_secret_reference IS NULL
        OR previous_webhook_secret_reference ~ '^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,254}$'
        OR (
            char_length(previous_webhook_secret_reference) <= 32768
            AND previous_webhook_secret_reference ~ '^enc://[A-Za-z0-9_-]+$'
        )
    ),
    CONSTRAINT provider_accounts_webhook_rotation_shape_check CHECK (
        (previous_webhook_secret_reference IS NULL AND webhook_rotation_expires_at IS NULL)
        OR (previous_webhook_secret_reference IS NOT NULL AND webhook_rotation_expires_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_readiness_status_check CHECK (
        readiness_status IN ('unchecked', 'ready', 'action_required')
    ),
    CONSTRAINT provider_accounts_readiness_shape_check CHECK (
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
    CONSTRAINT provider_accounts_enabled_readiness_check CHECK (
        NOT enabled
        OR (
            readiness_status = 'ready'
            AND readiness_valid_until IS NOT NULL
            AND readiness_reconcile_at IS NOT NULL
        )
    ),
    CONSTRAINT provider_accounts_readiness_validity_check CHECK (
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
    CONSTRAINT provider_accounts_readiness_lock_shape_check CHECK (
        (readiness_locked_by IS NULL AND readiness_locked_at IS NULL)
        OR (readiness_locked_by IS NOT NULL AND readiness_locked_at IS NOT NULL)
    ),
    CONSTRAINT provider_accounts_readiness_attempts_check CHECK (
        readiness_reconcile_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT provider_accounts_readiness_error_length_check CHECK (
        readiness_last_error IS NULL OR length(readiness_last_error) BETWEEN 1 AND 2000
    )
);

CREATE TABLE payments.payment_attempts (
    id                     UUID                            NOT NULL PRIMARY KEY,
    store_id               UUID                            NOT NULL,
    order_id               UUID                            NOT NULL,
    shopper_id             UUID                            NOT NULL,
    provider_account_id    UUID                            NOT NULL,
    amount_minor           BIGINT                          NOT NULL,
    currency               CHAR(3)                         NOT NULL,
    status                 payments.payment_attempt_status NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, id, shopper_id),
    UNIQUE (store_id, id, currency),
    UNIQUE (provider_account_id, provider_reference),
    FOREIGN KEY (store_id, order_id, shopper_id)
        REFERENCES sales.orders(store_id, id, shopper_id),
    FOREIGN KEY (store_id, order_id, currency)
        REFERENCES sales.orders(store_id, id, currency),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES payments.provider_accounts(store_id, id),
    CONSTRAINT payment_attempts_amount_positive_check CHECK (amount_minor > 0),
    CONSTRAINT payment_attempts_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT payment_attempts_provider_reference_length_check CHECK (
        provider_reference IS NULL
        OR length(trim(provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT payment_attempts_failure_shape_check CHECK (
        (status = 'failed' AND failure_code IS NOT NULL)
        OR (status <> 'failed' AND failure_code IS NULL)
    )
);

CREATE TABLE payments.refunds (
    id                     UUID                    NOT NULL PRIMARY KEY,
    store_id               UUID                    NOT NULL,
    payment_attempt_id     UUID                    NOT NULL,
    amount_minor           BIGINT                  NOT NULL,
    currency               CHAR(3)                 NOT NULL,
    status                 payments.refund_status  NOT NULL DEFAULT 'pending',
    provider_reference     TEXT,
    failure_code           TEXT,
    created_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at             TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (payment_attempt_id, provider_reference),
    FOREIGN KEY (store_id, payment_attempt_id, currency)
        REFERENCES payments.payment_attempts(store_id, id, currency),
    CONSTRAINT refunds_amount_positive_check CHECK (amount_minor > 0),
    CONSTRAINT refunds_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT refunds_provider_reference_length_check CHECK (
        provider_reference IS NULL
        OR length(trim(provider_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT refunds_failure_shape_check CHECK (
        (status = 'failed' AND failure_code IS NOT NULL)
        OR (status <> 'failed' AND failure_code IS NULL)
    )
);

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE TABLE fulfillment.fulfillments (
    id                   UUID                           NOT NULL PRIMARY KEY,
    store_id             UUID                           NOT NULL,
    order_id             UUID                           NOT NULL,
    status               fulfillment.fulfillment_status NOT NULL DEFAULT 'pending',
    carrier              TEXT,
    tracking_number      TEXT,
    shipped_at           TIMESTAMPTZ,
    delivered_at         TIMESTAMPTZ,
    cancelled_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, carrier, tracking_number),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id),
    CONSTRAINT fulfillments_tracking_shape_check CHECK (
        (status = 'pending' AND carrier IS NULL AND tracking_number IS NULL
            AND shipped_at IS NULL AND delivered_at IS NULL AND cancelled_at IS NULL)
        OR (status = 'shipped' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND shipped_at IS NOT NULL AND delivered_at IS NULL AND cancelled_at IS NULL)
        OR (status = 'delivered' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND shipped_at IS NOT NULL AND delivered_at IS NOT NULL AND cancelled_at IS NULL)
        OR (status = 'cancelled' AND shipped_at IS NULL AND delivered_at IS NULL
            AND cancelled_at IS NOT NULL)
    ),
    CONSTRAINT fulfillments_carrier_length_check CHECK (
        carrier IS NULL OR length(trim(carrier)) BETWEEN 1 AND 64
    ),
    CONSTRAINT fulfillments_tracking_number_length_check CHECK (
        tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255
    )
);

CREATE TABLE fulfillment.fulfillment_lines (
    store_id             UUID    NOT NULL,
    fulfillment_id       UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             INTEGER NOT NULL,

    PRIMARY KEY (store_id, fulfillment_id, product_variant_id),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(store_id, id),
    CONSTRAINT fulfillment_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999)
);

CREATE TABLE fulfillment.shipping_quote_requests (
    id                    UUID        NOT NULL PRIMARY KEY,
    store_id              UUID        NOT NULL,
    fulfillment_id        UUID        NOT NULL,
    provider_account_id   UUID        NOT NULL,
    idempotency_key       TEXT        NOT NULL,
    request_fingerprint   BYTEA       NOT NULL,
    length_millimetres    INTEGER     NOT NULL,
    width_millimetres     INTEGER     NOT NULL,
    height_millimetres    INTEGER     NOT NULL,
    weight_grams          INTEGER     NOT NULL,
    state                 TEXT        NOT NULL DEFAULT 'pending',
    expires_at            TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, idempotency_key),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(store_id, id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES fulfillment.shipping_provider_accounts(store_id, id),
    CONSTRAINT shipping_quote_requests_idempotency_length_check CHECK (
        length(idempotency_key) BETWEEN 1 AND 128
    ),
    CONSTRAINT shipping_quote_requests_fingerprint_length_check CHECK (
        octet_length(request_fingerprint) = 32
    ),
    CONSTRAINT shipping_quote_requests_parcel_check CHECK (
        length_millimetres BETWEEN 1 AND 10000
        AND width_millimetres BETWEEN 1 AND 10000
        AND height_millimetres BETWEEN 1 AND 10000
        AND weight_grams BETWEEN 1 AND 1000000
    ),
    CONSTRAINT shipping_quote_requests_state_check CHECK (state IN ('pending', 'completed')),
    CONSTRAINT shipping_quote_requests_completion_check CHECK (
        (state = 'pending' AND expires_at IS NULL)
        OR (state = 'completed' AND expires_at IS NOT NULL)
    )
);

CREATE TABLE fulfillment.shipping_rate_quotes (
    id                            UUID        NOT NULL PRIMARY KEY,
    store_id                      UUID        NOT NULL,
    quote_request_id              UUID        NOT NULL,
    provider_shipment_reference   TEXT        NOT NULL,
    provider_rate_reference       TEXT        NOT NULL,
    carrier                       TEXT        NOT NULL,
    service                       TEXT        NOT NULL,
    amount_minor                  BIGINT      NOT NULL,
    currency                      CHAR(3)     NOT NULL,
    estimated_delivery_days       SMALLINT,
    guaranteed                    BOOLEAN     NOT NULL,
    expires_at                    TIMESTAMPTZ NOT NULL,
    created_at                    TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, quote_request_id, provider_rate_reference),
    FOREIGN KEY (store_id, quote_request_id)
        REFERENCES fulfillment.shipping_quote_requests(store_id, id),
    CONSTRAINT shipping_rate_quotes_shipment_reference_length_check CHECK (
        length(trim(provider_shipment_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_rate_quotes_rate_reference_length_check CHECK (
        length(trim(provider_rate_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_rate_quotes_carrier_length_check CHECK (
        length(trim(carrier)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_rate_quotes_service_length_check CHECK (
        length(trim(service)) BETWEEN 1 AND 120
    ),
    CONSTRAINT shipping_rate_quotes_amount_nonnegative_check CHECK (amount_minor >= 0),
    CONSTRAINT shipping_rate_quotes_currency_format_check CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT shipping_rate_quotes_delivery_days_check CHECK (
        estimated_delivery_days IS NULL OR estimated_delivery_days BETWEEN 0 AND 365
    )
);

CREATE TABLE fulfillment.shipping_labels (
    id                              UUID        NOT NULL PRIMARY KEY,
    store_id                        UUID        NOT NULL,
    fulfillment_id                  UUID        NOT NULL,
    provider_account_id             UUID        NOT NULL,
    rate_quote_id                   UUID        NOT NULL,
    purchase_idempotency_key        TEXT        NOT NULL,
    purchase_request_fingerprint    BYTEA       NOT NULL,
    purchase_state                  TEXT        NOT NULL DEFAULT 'purchasing',
    provider_shipment_reference     TEXT        NOT NULL,
    provider_rate_reference         TEXT        NOT NULL,
    carrier                         TEXT,
    tracking_number                 TEXT,
    provider_tracker_reference      TEXT,
    label_url                       TEXT,
    label_media_type                TEXT,
    cancellation_idempotency_key    TEXT,
    cancellation_request_fingerprint BYTEA,
    cancellation_status             TEXT,
    cancellation_reconcile_at       TIMESTAMPTZ,
    cancellation_locked_by          UUID,
    cancellation_locked_at          TIMESTAMPTZ,
    cancellation_attempts           INTEGER     NOT NULL DEFAULT 0,
    cancellation_last_error         TEXT,
    tracking_status                 TEXT,
    tracking_status_detail          TEXT,
    estimated_delivery_at           TIMESTAMPTZ,
    tracking_observed_at            TIMESTAMPTZ,
    next_tracking_refresh_at        TIMESTAMPTZ,
    tracking_locked_by              UUID,
    tracking_locked_at              TIMESTAMPTZ,
    tracking_attempts               INTEGER     NOT NULL DEFAULT 0,
    tracking_last_error             TEXT,
    purchased_at                    TIMESTAMPTZ,
    cancellation_requested_at       TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, fulfillment_id),
    CONSTRAINT shipping_labels_purchase_idempotency_key
        UNIQUE (store_id, purchase_idempotency_key),
    CONSTRAINT shipping_labels_cancellation_idempotency_key
        UNIQUE (store_id, cancellation_idempotency_key),
    UNIQUE (carrier, tracking_number),
    FOREIGN KEY (store_id, fulfillment_id)
        REFERENCES fulfillment.fulfillments(store_id, id),
    FOREIGN KEY (store_id, provider_account_id)
        REFERENCES fulfillment.shipping_provider_accounts(store_id, id),
    FOREIGN KEY (store_id, rate_quote_id)
        REFERENCES fulfillment.shipping_rate_quotes(store_id, id),
    CONSTRAINT shipping_labels_purchase_idempotency_length_check CHECK (
        length(purchase_idempotency_key) BETWEEN 1 AND 128
    ),
    CONSTRAINT shipping_labels_purchase_fingerprint_length_check CHECK (
        octet_length(purchase_request_fingerprint) = 32
    ),
    CONSTRAINT shipping_labels_purchase_state_check CHECK (
        purchase_state IN ('purchasing', 'purchased')
    ),
    CONSTRAINT shipping_labels_provider_reference_length_check CHECK (
        length(trim(provider_shipment_reference)) BETWEEN 1 AND 255
        AND length(trim(provider_rate_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_purchase_shape_check CHECK (
        (purchase_state = 'purchasing' AND carrier IS NULL AND tracking_number IS NULL
            AND label_url IS NULL AND label_media_type IS NULL AND purchased_at IS NULL)
        OR (purchase_state = 'purchased' AND carrier IS NOT NULL AND tracking_number IS NOT NULL
            AND label_url IS NOT NULL AND label_media_type IS NOT NULL AND purchased_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_carrier_length_check CHECK (
        carrier IS NULL OR length(trim(carrier)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_labels_tracking_number_length_check CHECK (
        tracking_number IS NULL OR length(trim(tracking_number)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_tracker_reference_length_check CHECK (
        provider_tracker_reference IS NULL
        OR length(trim(provider_tracker_reference)) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_label_url_check CHECK (
        label_url IS NULL OR (length(label_url) BETWEEN 9 AND 2048 AND label_url ~ '^https://')
    ),
    CONSTRAINT shipping_labels_media_type_length_check CHECK (
        label_media_type IS NULL OR length(trim(label_media_type)) BETWEEN 1 AND 100
    ),
    CONSTRAINT shipping_labels_cancellation_shape_check CHECK (
        (cancellation_idempotency_key IS NULL AND cancellation_request_fingerprint IS NULL
            AND cancellation_status IS NULL AND cancellation_requested_at IS NULL)
        OR (length(cancellation_idempotency_key) BETWEEN 1 AND 128
            AND octet_length(cancellation_request_fingerprint) = 32
            AND cancellation_status IN ('submitted', 'cancelled', 'rejected', 'not_available')
            AND cancellation_requested_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_cancellation_lock_shape_check CHECK (
        (cancellation_locked_by IS NULL AND cancellation_locked_at IS NULL)
        OR (cancellation_locked_by IS NOT NULL AND cancellation_locked_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_cancellation_attempts_check CHECK (
        cancellation_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT shipping_labels_cancellation_error_length_check CHECK (
        cancellation_last_error IS NULL OR length(cancellation_last_error) BETWEEN 1 AND 2000
    ),
    CONSTRAINT shipping_labels_tracking_status_check CHECK (
        tracking_status IS NULL
        OR tracking_status IN ('pre_transit', 'in_transit', 'out_for_delivery', 'delivered', 'failure', 'unknown')
    ),
    CONSTRAINT shipping_labels_tracking_detail_length_check CHECK (
        tracking_status_detail IS NULL OR length(tracking_status_detail) BETWEEN 1 AND 255
    ),
    CONSTRAINT shipping_labels_tracking_observation_shape_check CHECK (
        (tracking_status IS NULL AND tracking_observed_at IS NULL)
        OR (tracking_status IS NOT NULL AND tracking_observed_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_tracking_lock_shape_check CHECK (
        (tracking_locked_by IS NULL AND tracking_locked_at IS NULL)
        OR (tracking_locked_by IS NOT NULL AND tracking_locked_at IS NOT NULL)
    ),
    CONSTRAINT shipping_labels_tracking_attempts_check CHECK (
        tracking_attempts BETWEEN 0 AND 31
    ),
    CONSTRAINT shipping_labels_tracking_error_length_check CHECK (
        tracking_last_error IS NULL OR length(tracking_last_error) BETWEEN 1 AND 2000
    )
);

CREATE TABLE fulfillment.returns (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    order_id             UUID                      NOT NULL,
    status               fulfillment.return_status NOT NULL DEFAULT 'requested',
    refund_id            UUID,
    refund_amount_minor  BIGINT                    NOT NULL,
    currency             CHAR(3)                   NOT NULL,
    requested_at         TIMESTAMPTZ               NOT NULL,
    authorized_at        TIMESTAMPTZ,
    received_at          TIMESTAMPTZ,
    completed_at         TIMESTAMPTZ,
    rejected_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, order_id)
        REFERENCES sales.orders(store_id, id),
    FOREIGN KEY (store_id, order_id, currency)
        REFERENCES sales.orders(store_id, id, currency),
    FOREIGN KEY (store_id, refund_id)
        REFERENCES payments.refunds(store_id, id),
    CONSTRAINT returns_status_timestamps_check CHECK (
        (status = 'requested' AND authorized_at IS NULL AND received_at IS NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'authorized' AND authorized_at IS NOT NULL AND received_at IS NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'received' AND authorized_at IS NOT NULL AND received_at IS NOT NULL
            AND completed_at IS NULL AND rejected_at IS NULL)
        OR (status = 'completed' AND authorized_at IS NOT NULL AND received_at IS NOT NULL
            AND completed_at IS NOT NULL AND rejected_at IS NULL)
        OR (status = 'rejected' AND received_at IS NULL AND completed_at IS NULL
            AND rejected_at IS NOT NULL)
    ),
    CONSTRAINT returns_refund_amount_nonnegative_check CHECK (refund_amount_minor >= 0),
    CONSTRAINT returns_currency_format_check CHECK (currency ~ '^[A-Z]{3}$')
);

CREATE TABLE fulfillment.return_lines (
    store_id             UUID                           NOT NULL,
    return_id            UUID                           NOT NULL,
    product_variant_id   UUID                           NOT NULL,
    inventory_location_id UUID,
    quantity             INTEGER                        NOT NULL,
    refund_amount_minor  BIGINT                         NOT NULL,
    disposition          fulfillment.return_disposition,

    PRIMARY KEY (store_id, return_id, product_variant_id),
    FOREIGN KEY (store_id, return_id)
        REFERENCES fulfillment.returns(store_id, id),
    FOREIGN KEY (store_id, inventory_location_id)
        REFERENCES inventory.inventory_locations(store_id, id),
    CONSTRAINT return_lines_quantity_range_check CHECK (quantity BETWEEN 1 AND 999),
    CONSTRAINT return_lines_refund_amount_nonnegative_check CHECK (refund_amount_minor >= 0),
    CONSTRAINT return_lines_restock_location_check CHECK (
        disposition <> 'restock' OR inventory_location_id IS NOT NULL
    )
);

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE INDEX customers_store_created_idx
    ON sales.customers (store_id, created_at DESC, id DESC);

CREATE INDEX customer_addresses_customer_idx
    ON sales.customer_addresses (store_id, customer_id, created_at, id);

CREATE INDEX customer_shopper_links_history_idx
    ON sales.customer_shopper_links (store_id, customer_id, sales_channel_id, shopper_id
    );

CREATE INDEX carts_channel_updated_idx
    ON sales.carts (store_id,
        sales_channel_id,
        status,
        updated_at DESC,
        id DESC
    );

CREATE INDEX cart_lines_variant_lookup_idx
    ON sales.cart_lines (store_id, product_variant_id, cart_id);

CREATE INDEX checkouts_channel_created_idx
    ON sales.checkouts (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX checkouts_expiry_claim_idx
    ON sales.checkouts (expires_at, id)
    WHERE status = 'pending';

CREATE INDEX orders_channel_created_idx
    ON sales.orders (store_id,
        sales_channel_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX orders_customer_created_idx
    ON sales.orders (store_id, customer_id, created_at DESC, id DESC
    ) WHERE customer_id IS NOT NULL;

CREATE INDEX order_transitions_order_time_idx
    ON sales.order_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

CREATE INDEX order_fulfillment_transitions_order_idx
    ON sales.order_fulfillment_transitions (store_id,
        order_id,
        occurred_at,
        id
    );

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE INDEX provider_accounts_store_created_idx
    ON payments.provider_accounts (store_id, created_at DESC, id DESC);

CREATE INDEX provider_accounts_readiness_due_idx
    ON payments.provider_accounts (readiness_reconcile_at, id)
    WHERE enabled;

CREATE INDEX payment_attempts_order_created_idx
    ON payments.payment_attempts (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX refunds_attempt_created_idx
    ON payments.refunds (store_id,
        payment_attempt_id,
        created_at DESC,
        id DESC
    );

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE INDEX shipping_services_quote_idx
    ON fulfillment.shipping_services (store_id,
        currency,
        status,
        id
    );

CREATE INDEX shipping_provider_accounts_store_created_idx
    ON fulfillment.shipping_provider_accounts (store_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX shipping_service_regions_quote_idx
    ON fulfillment.shipping_service_regions (store_id,
        country_code,
        shipping_service_id
    );

CREATE INDEX fulfillments_order_created_idx
    ON fulfillment.fulfillments (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX fulfillment_lines_variant_idx
    ON fulfillment.fulfillment_lines (store_id,
        product_variant_id,
        fulfillment_id
    );

CREATE INDEX shipping_quote_requests_fulfillment_created_idx
    ON fulfillment.shipping_quote_requests (store_id,
        fulfillment_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX shipping_rate_quotes_request_expiry_idx
    ON fulfillment.shipping_rate_quotes (store_id,
        quote_request_id,
        expires_at,
        id
    );

CREATE INDEX shipping_labels_tracking_due_idx
    ON fulfillment.shipping_labels (next_tracking_refresh_at, id)
    WHERE purchase_state = 'purchased' AND next_tracking_refresh_at IS NOT NULL;

CREATE INDEX shipping_labels_cancellation_due_idx
    ON fulfillment.shipping_labels (cancellation_reconcile_at, id)
    WHERE cancellation_status = 'submitted' AND cancellation_reconcile_at IS NOT NULL;

CREATE INDEX returns_order_created_idx
    ON fulfillment.returns (store_id,
        order_id,
        created_at DESC,
        id DESC
    );

CREATE INDEX return_lines_variant_idx
    ON fulfillment.return_lines (store_id,
        product_variant_id,
        return_id
    );

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE FUNCTION sales.claim_expired_checkouts(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    inventory_reservation_id UUID
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT checkout.id
        FROM sales.checkouts AS checkout
        WHERE checkout.status = 'pending'
          AND checkout.expires_at <= claimed_at
          AND (
              checkout.expiry_locked_at IS NULL
              OR checkout.expiry_locked_at <= stale_before
          )
        ORDER BY checkout.expires_at, checkout.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 500), 1)
    )
    UPDATE sales.checkouts AS checkout
       SET expiry_locked_by = worker_id,
           expiry_locked_at = claimed_at
      FROM claimable
     WHERE checkout.id = claimable.id
    RETURNING checkout.id, checkout.store_id,
              checkout.inventory_reservation_id;
$$;

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE FUNCTION payments.provider_readiness_metrics()
RETURNS TABLE (
    due BIGINT,
    retrying BIGINT,
    expiring_within_six_hours BIGINT,
    action_required BIGINT
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_reconcile_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_reconcile_attempts > 0
           ),
           count(*) FILTER (
               WHERE account.enabled
                 AND account.readiness_valid_until <= CURRENT_TIMESTAMP + INTERVAL '6 hours'
           ),
           count(*) FILTER (WHERE account.readiness_status = 'action_required')
      FROM payments.provider_accounts AS account;
$$;

CREATE FUNCTION payments.resolve_provider_account(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
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
    FROM payments.provider_accounts AS account
    WHERE account.provider = requested_provider
      AND account.external_account_reference = requested_external_account_reference;
$$;

CREATE FUNCTION payments.resolve_provider_webhook_secret_references(
    requested_provider                   TEXT,
    requested_external_account_reference TEXT
)
RETURNS TABLE (secret_reference TEXT)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT candidate.secret_reference
    FROM payments.provider_accounts AS account
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
      AND account.external_account_reference = requested_external_account_reference
      AND candidate.secret_reference IS NOT NULL
    ORDER BY candidate.priority;
$$;

CREATE FUNCTION payments.claim_provider_readiness_checks(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    provider_account_id UUID,
    store_id UUID,
    provider TEXT,
    external_account_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE payments.provider_accounts AS account
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
        FROM payments.provider_accounts AS account
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
    UPDATE payments.provider_accounts AS account
       SET readiness_locked_by = worker_id,
           readiness_locked_at = claimed_at,
           readiness_reconcile_attempts = least(account.readiness_reconcile_attempts, 30) + 1
      FROM claimable
     WHERE account.id = claimable.id
    RETURNING account.id, account.store_id, account.provider,
              account.external_account_reference, account.credential_secret_reference,
              account.readiness_reconcile_attempts;
$$;

CREATE FUNCTION payments.finish_provider_readiness_check(
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
    UPDATE payments.provider_accounts AS account
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

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE FUNCTION fulfillment.shipping_tracking_metrics()
RETURNS TABLE (
    due BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_due_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE label.next_tracking_refresh_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (WHERE label.tracking_locked_by IS NOT NULL),
           count(*) FILTER (
               WHERE label.next_tracking_refresh_at IS NULL
                 AND label.tracking_attempts >= 8
                 AND label.tracking_last_error IS NOT NULL
           ),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(label.next_tracking_refresh_at)
                            FILTER (WHERE label.next_tracking_refresh_at <= CURRENT_TIMESTAMP))
               ),
               0
           )::DOUBLE PRECISION
      FROM fulfillment.shipping_labels AS label
     WHERE label.purchase_state = 'purchased'
       AND label.provider_tracker_reference IS NOT NULL;
$$;

CREATE FUNCTION fulfillment.shipping_cancellation_metrics()
RETURNS TABLE (
    due BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    oldest_due_seconds DOUBLE PRECISION
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT count(*) FILTER (
               WHERE label.cancellation_reconcile_at <= CURRENT_TIMESTAMP
           ),
           count(*) FILTER (WHERE label.cancellation_locked_by IS NOT NULL),
           count(*) FILTER (
               WHERE label.cancellation_reconcile_at IS NULL
                 AND label.cancellation_attempts >= 8
                 AND label.cancellation_last_error IS NOT NULL
           ),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       (min(label.cancellation_reconcile_at)
                            FILTER (WHERE label.cancellation_reconcile_at <= CURRENT_TIMESTAMP))
               ),
               0
           )::DOUBLE PRECISION
      FROM fulfillment.shipping_labels AS label
     WHERE label.cancellation_status = 'submitted';
$$;

CREATE FUNCTION fulfillment.claim_shipping_tracking(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    store_id UUID,
    fulfillment_id UUID,
    provider TEXT,
    provider_tracker_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT label.id
        FROM fulfillment.shipping_labels AS label
        WHERE label.purchase_state = 'purchased'
          AND label.provider_tracker_reference IS NOT NULL
          AND label.next_tracking_refresh_at <= claimed_at
          AND (
              label.tracking_locked_at IS NULL
              OR label.tracking_locked_at <= stale_before
          )
        ORDER BY label.next_tracking_refresh_at, label.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE fulfillment.shipping_labels AS label
       SET tracking_locked_by = worker_id,
           tracking_locked_at = claimed_at,
           tracking_attempts = least(label.tracking_attempts, 30) + 1
      FROM claimable,
           fulfillment.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_tracker_reference,
              account.credential_secret_reference, label.tracking_attempts;
$$;

CREATE FUNCTION fulfillment.claim_shipping_cancellations(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    label_id UUID,
    store_id UUID,
    fulfillment_id UUID,
    provider TEXT,
    provider_shipment_reference TEXT,
    credential_secret_reference TEXT,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH claimable AS (
        SELECT label.id
        FROM fulfillment.shipping_labels AS label
        WHERE label.cancellation_status = 'submitted'
          AND label.cancellation_reconcile_at <= claimed_at
          AND (
              label.cancellation_locked_at IS NULL
              OR label.cancellation_locked_at <= stale_before
          )
        ORDER BY label.cancellation_reconcile_at, label.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE fulfillment.shipping_labels AS label
       SET cancellation_locked_by = worker_id,
           cancellation_locked_at = claimed_at,
           cancellation_attempts = least(label.cancellation_attempts, 30) + 1
      FROM claimable,
           fulfillment.shipping_provider_accounts AS account
     WHERE label.id = claimable.id
       AND account.id = label.provider_account_id
       AND account.store_id = label.store_id
    RETURNING label.id, label.store_id, label.fulfillment_id,
              account.provider, label.provider_shipment_reference,
              account.credential_secret_reference, label.cancellation_attempts;
$$;

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

ALTER TABLE sales.customers ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.customer_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.customer_shopper_links ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.carts ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.cart_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkouts ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_contacts ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_tax_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_promotion_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.orders ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_contacts ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_addresses ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_tax_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_promotion_calculations ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_transitions ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_fulfillment_transitions ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.checkout_shipping_selections ENABLE ROW LEVEL SECURITY;

ALTER TABLE sales.order_shipping_selections ENABLE ROW LEVEL SECURITY;

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

ALTER TABLE payments.provider_accounts ENABLE ROW LEVEL SECURITY;

ALTER TABLE payments.payment_attempts ENABLE ROW LEVEL SECURITY;

ALTER TABLE payments.refunds ENABLE ROW LEVEL SECURITY;

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

ALTER TABLE fulfillment.fulfillments ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_services ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_provider_accounts ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_service_regions ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.fulfillment_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_quote_requests ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_rate_quotes ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.shipping_labels ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.returns ENABLE ROW LEVEL SECURITY;

ALTER TABLE fulfillment.return_lines ENABLE ROW LEVEL SECURITY;

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE POLICY store_isolation ON sales.carts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.customers
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.customer_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.customer_shopper_links
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.cart_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkouts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_contacts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_tax_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_promotion_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.orders
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_contacts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_addresses
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_tax_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_promotion_calculations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_transitions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_fulfillment_transitions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.checkout_shipping_selections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON sales.order_shipping_selections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE POLICY store_isolation ON payments.provider_accounts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON payments.payment_attempts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON payments.refunds
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

CREATE POLICY store_isolation ON fulfillment.fulfillments
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_services
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_provider_accounts
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_service_regions
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.fulfillment_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_quote_requests
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_rate_quotes
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.shipping_labels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.returns
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON fulfillment.return_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

COMMENT ON INDEX sales.checkouts_expiry_claim_idx IS
    'Supports the cross-tenant SECURITY DEFINER expiry scheduler claim path';

REVOKE ALL ON FUNCTION sales.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION sales.claim_expired_checkouts(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA sales TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON sales.customer_shopper_links FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON sales.checkout_contacts, sales.checkout_addresses, sales.checkout_lines,
       sales.checkout_tax_calculations,
       sales.checkout_promotion_calculations FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON sales.order_contacts, sales.order_addresses, sales.order_lines,
       sales.order_tax_calculations,
       sales.order_promotion_calculations, sales.order_transitions,
       sales.order_fulfillment_transitions
    FROM chaos_runtime;

REVOKE DELETE ON sales.checkouts, sales.orders FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA sales TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA sales
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA sales
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON sales.checkout_shipping_selections FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON sales.order_shipping_selections FROM chaos_runtime;

-- ============================================================================
-- SCHEMA: payments
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

REVOKE ALL ON FUNCTION payments.resolve_provider_account(TEXT, TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION payments.resolve_provider_webhook_secret_references(TEXT, TEXT) FROM PUBLIC;

REVOKE ALL ON FUNCTION payments.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION payments.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
) FROM PUBLIC;

REVOKE ALL ON FUNCTION payments.provider_readiness_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION payments.resolve_provider_account(TEXT, TEXT) TO chaos_runtime;

GRANT EXECUTE
    ON FUNCTION payments.resolve_provider_webhook_secret_references(TEXT, TEXT) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION payments.claim_provider_readiness_checks(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION payments.finish_provider_readiness_check(
    UUID, UUID, BOOLEAN, BOOLEAN, JSONB, TIMESTAMPTZ, TEXT
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION payments.provider_readiness_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA payments TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA payments TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA payments
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA payments
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

-- ============================================================================
-- SCHEMA: fulfillment
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

REVOKE ALL ON FUNCTION fulfillment.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION fulfillment.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION fulfillment.shipping_tracking_metrics() FROM PUBLIC;

REVOKE ALL ON FUNCTION fulfillment.shipping_cancellation_metrics() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION fulfillment.claim_shipping_tracking(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION fulfillment.claim_shipping_cancellations(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION fulfillment.shipping_tracking_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION fulfillment.shipping_cancellation_metrics() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA fulfillment TO chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA fulfillment TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA fulfillment
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA fulfillment
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

-- ============================================================================
-- SCHEMA: sales
-- Dependency-preserving section; statement order retained from the original migration.
-- ============================================================================

GRANT USAGE ON SCHEMA sales, payments, fulfillment TO chaos_runtime;
