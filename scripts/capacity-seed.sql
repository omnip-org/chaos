\set ON_ERROR_STOP on

-- Run only against a dedicated capacity-test database.
-- Required psql variables: dataset_size, publishable_key_digest, key_identifier, key_suffix.

BEGIN;

SELECT
    uuidv7() AS user_id,
    uuidv7() AS merchant_account_id,
    uuidv7() AS store_id,
    uuidv7() AS sales_channel_id,
    uuidv7() AS api_key_id,
    uuidv7() AS price_list_id,
    uuidv7() AS inventory_location_id,
    uuidv7() AS shipping_service_id,
    uuidv7() AS tax_rule_id
\gset

INSERT INTO identity.users (id, email)
VALUES (:'user_id', 'capacity-owner@example.invalid');

INSERT INTO merchant.merchant_accounts (id, slug, display_name)
VALUES (:'merchant_account_id', 'capacity-baseline', 'Capacity Baseline');

INSERT INTO merchant.merchant_account_memberships (merchant_account_id, user_id, role)
VALUES (:'merchant_account_id', :'user_id', 'owner');

INSERT INTO merchant.stores (
    id, merchant_account_id, code, name, default_region, default_currency, status
)
VALUES (
    :'store_id', :'merchant_account_id', 'capacity', 'Capacity', 'SG', 'USD', 'active'
);

INSERT INTO merchant.store_currencies (merchant_account_id, store_id, currency)
VALUES (:'merchant_account_id', :'store_id', 'USD');

INSERT INTO merchant.sales_channels (
    id, merchant_account_id, store_id, code, name, kind, status, is_default
)
VALUES (
    :'sales_channel_id', :'merchant_account_id', :'store_id',
    'web', 'Web', 'web', 'active', true
);

INSERT INTO merchant.api_keys (
    id, merchant_account_id, store_id, sales_channel_id, key_identifier,
    secret_digest, display_suffix, name, class, mode, created_by_user_id
)
VALUES (
    :'api_key_id', :'merchant_account_id', :'store_id', :'sales_channel_id',
    :'key_identifier', decode(:'publishable_key_digest', 'hex'), :'key_suffix',
    'Capacity Harness', 'publishable', 'test', :'user_id'
);

INSERT INTO merchant.api_key_scopes (merchant_account_id, api_key_id, scope)
VALUES
    (:'merchant_account_id', :'api_key_id', 'carts:write'),
    (:'merchant_account_id', :'api_key_id', 'checkout:write');

INSERT INTO pricing.price_lists (
    id, merchant_account_id, store_id, code, name, currency, tax_inclusive, status
)
VALUES (
    :'price_list_id', :'merchant_account_id', :'store_id',
    'capacity-usd', 'Capacity USD', 'USD', false, 'active'
);

CREATE TEMP TABLE capacity_products ON COMMIT DROP AS
SELECT sequence, uuidv7() AS product_id, uuidv7() AS variant_id
FROM generate_series(1, :dataset_size) AS sequence;

INSERT INTO catalog.products (
    id, merchant_account_id, store_id, handle, title, description, status
)
SELECT
    product_id, :'merchant_account_id', :'store_id',
    'capacity-product-' || sequence, 'Capacity Product ' || sequence,
    'Synthetic capacity-test fixture', 'active'
FROM capacity_products;

INSERT INTO catalog.product_variants (
    id, merchant_account_id, store_id, product_id, title, sku,
    status, requires_shipping, track_inventory
)
SELECT
    variant_id, :'merchant_account_id', :'store_id', product_id,
    'Default', 'CAPACITY-' || sequence, 'active', true, true
FROM capacity_products;

INSERT INTO catalog.product_publications (
    merchant_account_id, store_id, product_id, sales_channel_id
)
SELECT :'merchant_account_id', :'store_id', product_id, :'sales_channel_id'
FROM capacity_products;

INSERT INTO pricing.prices (
    id, merchant_account_id, store_id, price_list_id, product_variant_id, amount_minor
)
SELECT
    uuidv7(), :'merchant_account_id', :'store_id', :'price_list_id', variant_id, 1000
FROM capacity_products;

INSERT INTO inventory.inventory_locations (
    id, merchant_account_id, store_id, code, name
)
VALUES (
    :'inventory_location_id', :'merchant_account_id', :'store_id',
    'capacity', 'Capacity'
);

INSERT INTO inventory.stock_items (
    id, merchant_account_id, store_id, inventory_location_id,
    product_variant_id, on_hand_quantity
)
SELECT
    uuidv7(), :'merchant_account_id', :'store_id', :'inventory_location_id',
    variant_id, 1000000
FROM capacity_products
WHERE sequence = 1;

INSERT INTO fulfillment.shipping_services (
    id, merchant_account_id, store_id, code, name, amount_minor, currency,
    estimated_min_days, estimated_max_days
)
VALUES (
    :'shipping_service_id', :'merchant_account_id', :'store_id',
    'capacity-standard', 'Capacity Standard', 500, 'USD', 2, 5
);

INSERT INTO fulfillment.shipping_service_regions (
    merchant_account_id, store_id, shipping_service_id, country_code
)
VALUES (:'merchant_account_id', :'store_id', :'shipping_service_id', 'SG');

INSERT INTO pricing.tax_rules (
    id, merchant_account_id, store_id, code, name, country_code, rate_basis_points
)
VALUES (
    :'tax_rule_id', :'merchant_account_id', :'store_id',
    'capacity-sg', 'Capacity SG', 'SG', 900
);

SELECT variant_id AS product_variant_id
FROM capacity_products
WHERE sequence = 1
\gset

COMMIT;

SELECT json_build_object(
    'merchant_account_id', :'merchant_account_id',
    'store_id', :'store_id',
    'sales_channel_id', :'sales_channel_id',
    'product_variant_id', :'product_variant_id',
    'shipping_service_id', :'shipping_service_id',
    'dataset_size', :dataset_size
) AS capacity_seed;
