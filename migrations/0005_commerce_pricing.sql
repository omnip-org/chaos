CREATE TYPE commerce.price_list_status AS ENUM ('draft', 'active', 'archived');

CREATE TABLE commerce.price_lists (
    id                   UUID                         NOT NULL PRIMARY KEY,
    store_id             UUID                         NOT NULL,
    code                 extensions.citext            NOT NULL,
    name                 TEXT                         NOT NULL,
    currency             CHAR(3)                      NOT NULL,
    status               commerce.price_list_status   NOT NULL DEFAULT 'draft',
    starts_at            TIMESTAMPTZ,
    ends_at              TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT price_lists_store_id_code_key            UNIQUE (store_id, code),
    CONSTRAINT price_lists_store_id_id_key              UNIQUE (store_id, id),
    CONSTRAINT price_lists_store_id_fkey                FOREIGN KEY (store_id) REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT price_lists_code_format_check            CHECK (code::text ~ '^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$'),
    CONSTRAINT price_lists_name_length_check            CHECK (length(trim(name)) BETWEEN 1 AND 120),
    CONSTRAINT price_lists_currency_format_check        CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT price_lists_validity_window_check        CHECK (starts_at IS NULL OR ends_at IS NULL OR ends_at > starts_at)
);

CREATE TABLE commerce.prices (
    id                   UUID         NOT NULL PRIMARY KEY,
    store_id             UUID         NOT NULL,
    price_list_id        UUID         NOT NULL,
    product_variant_id   UUID         NOT NULL,
    amount_minor         BIGINT       NOT NULL,
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT prices_store_id_price_list_id_product_variant_id_key    UNIQUE (store_id, price_list_id, product_variant_id),
    CONSTRAINT prices_store_id_price_list_fkey                         FOREIGN KEY (store_id, price_list_id) REFERENCES commerce.price_lists(store_id, id) ON DELETE CASCADE,
    CONSTRAINT prices_store_id_product_variant_fkey                    FOREIGN KEY (store_id, product_variant_id) REFERENCES commerce.product_variants(store_id, id),
    CONSTRAINT prices_amount_nonnegative_check                         CHECK (amount_minor >= 0)
);

ALTER TABLE commerce.price_lists ADD UNIQUE (store_id, id, currency);

CREATE INDEX price_lists_store_activation_idx ON commerce.price_lists (store_id, status, currency, starts_at, ends_at);
CREATE INDEX prices_variant_lookup_idx ON commerce.prices (store_id, product_variant_id, price_list_id);

-- A Store trades in exactly one currency (`stores.currency`); every Price
-- List it owns must match, so Cart/Order price resolution never has to
-- choose between Price Lists in different currencies.
CREATE FUNCTION commerce.check_price_list_currency()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.currency <> (SELECT store.currency FROM commerce.stores AS store WHERE store.id = NEW.store_id) THEN
        RAISE EXCEPTION 'price list currency must match the store currency'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER price_lists_currency_matches_store
    BEFORE INSERT OR UPDATE OF currency, store_id ON commerce.price_lists
    FOR EACH ROW
    EXECUTE FUNCTION commerce.check_price_list_currency();

REVOKE ALL ON FUNCTION commerce.check_price_list_currency() FROM PUBLIC;

ALTER TABLE commerce.price_lists ENABLE ROW LEVEL SECURITY;
ALTER TABLE commerce.prices ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.price_lists
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_isolation ON commerce.prices
    USING (store_id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (store_id = nullif(current_setting('app.store_id', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON commerce.price_lists,
       commerce.prices
    TO chaos_runtime;
