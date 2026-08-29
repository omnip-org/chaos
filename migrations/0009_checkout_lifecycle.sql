ALTER TYPE commerce.cart_status ADD VALUE IF NOT EXISTS 'checkout_pending';

ALTER TABLE commerce.stores
    ADD COLUMN shipping_policy_version BIGINT NOT NULL DEFAULT 1;

ALTER TABLE commerce.stores
    ADD CONSTRAINT stores_shipping_policy_version_check
    CHECK (shipping_policy_version >= 1);
