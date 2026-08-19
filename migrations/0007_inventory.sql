CREATE TYPE inventory.inventory_location_status AS ENUM ('active', 'archived');

CREATE TYPE inventory.inventory_reservation_status AS ENUM (
    'active',
    'released',
    'consumed',
    'expired'
);

CREATE TYPE inventory.stock_ledger_kind AS ENUM (
    'manual_adjustment',
    'reservation_created',
    'reservation_released',
    'reservation_consumed',
    'reservation_expired',
    'return_restock'
);

CREATE TABLE inventory.inventory_locations (
    id                   UUID                                    NOT NULL PRIMARY KEY,
    store_id             UUID                                    NOT NULL,
    code                 extensions.citext                       NOT NULL,
    name                 TEXT                                    NOT NULL,
    status               inventory.inventory_location_status     NOT NULL DEFAULT 'active',
    created_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT inventory_locations_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT inventory_locations_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE inventory.stock_items (
    id                    UUID        NOT NULL PRIMARY KEY,
    store_id              UUID        NOT NULL,
    inventory_location_id UUID        NOT NULL,
    product_variant_id    UUID        NOT NULL,
    on_hand_quantity      BIGINT      NOT NULL DEFAULT 0,
    reserved_quantity     BIGINT      NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, inventory_location_id, product_variant_id),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, inventory_location_id)
        REFERENCES inventory.inventory_locations(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT stock_items_on_hand_nonnegative_check CHECK (on_hand_quantity >= 0),
    CONSTRAINT stock_items_reserved_range_check CHECK (
        reserved_quantity >= 0 AND reserved_quantity <= on_hand_quantity
    )
);

CREATE TABLE inventory.inventory_reservations (
    id                   UUID                                      NOT NULL PRIMARY KEY,
    store_id             UUID                                      NOT NULL,
    sales_channel_id     UUID                                      NOT NULL,
    status               inventory.inventory_reservation_status    NOT NULL DEFAULT 'active',
    expires_at           TIMESTAMPTZ                               NOT NULL,
    closed_at            TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    CONSTRAINT inventory_reservations_expiration_check CHECK (expires_at > created_at),
    CONSTRAINT inventory_reservations_closure_check CHECK (
        (status = 'active' AND closed_at IS NULL)
        OR (status <> 'active' AND closed_at IS NOT NULL)
    )
);

CREATE TABLE inventory.inventory_reservation_lines (
    store_id             UUID    NOT NULL,
    reservation_id       UUID    NOT NULL,
    stock_item_id        UUID    NOT NULL,
    product_variant_id   UUID    NOT NULL,
    quantity             BIGINT  NOT NULL,

    PRIMARY KEY (store_id, reservation_id, stock_item_id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES inventory.stock_items(store_id, id),
    FOREIGN KEY (store_id, product_variant_id)
        REFERENCES catalog.product_variants(store_id, id),
    CONSTRAINT inventory_reservation_lines_quantity_positive_check CHECK (quantity > 0)
);

CREATE TABLE inventory.stock_ledger_entries (
    id                           UUID                        NOT NULL PRIMARY KEY,
    store_id                     UUID                        NOT NULL,
    stock_item_id                UUID                        NOT NULL,
    reservation_id               UUID,
    kind                         inventory.stock_ledger_kind NOT NULL,
    on_hand_delta_quantity       BIGINT                      NOT NULL,
    reserved_delta_quantity      BIGINT                      NOT NULL,
    resulting_on_hand_quantity   BIGINT                      NOT NULL,
    resulting_reserved_quantity  BIGINT                      NOT NULL,
    note                         TEXT,
    actor_user_id                UUID,
    created_at                   TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, stock_item_id)
        REFERENCES inventory.stock_items(store_id, id),
    FOREIGN KEY (store_id, reservation_id)
        REFERENCES inventory.inventory_reservations(store_id, id),
    FOREIGN KEY (actor_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT stock_ledger_entries_resulting_balance_check CHECK (
        resulting_on_hand_quantity >= 0
        AND resulting_reserved_quantity >= 0
        AND resulting_reserved_quantity <= resulting_on_hand_quantity
    ),
    CONSTRAINT stock_ledger_entries_note_length_check CHECK (
        note IS NULL OR length(trim(note)) BETWEEN 1 AND 500
    ),
    CONSTRAINT stock_ledger_entries_kind_deltas_check CHECK (
        (
            kind IN ('manual_adjustment', 'return_restock')
            AND reservation_id IS NULL
            AND (
                (kind = 'manual_adjustment' AND on_hand_delta_quantity <> 0)
                OR (kind = 'return_restock' AND on_hand_delta_quantity > 0)
            )
            AND reserved_delta_quantity = 0
        )
        OR (
            kind = 'reservation_created'
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity = 0
            AND reserved_delta_quantity > 0
        )
        OR (
            kind IN ('reservation_released', 'reservation_expired')
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity = 0
            AND reserved_delta_quantity < 0
        )
        OR (
            kind = 'reservation_consumed'
            AND reservation_id IS NOT NULL
            AND on_hand_delta_quantity < 0
            AND reserved_delta_quantity = on_hand_delta_quantity
        )
    )
);

CREATE INDEX inventory_locations_store_status_idx
    ON inventory.inventory_locations (store_id, status, created_at, id);

CREATE INDEX stock_items_variant_availability_idx
    ON inventory.stock_items (store_id,
        product_variant_id,
        inventory_location_id
    );

CREATE INDEX inventory_reservations_expiration_idx
    ON inventory.inventory_reservations (store_id,
        status,
        expires_at,
        id
    );

CREATE INDEX inventory_reservation_lines_stock_item_idx
    ON inventory.inventory_reservation_lines (store_id,
        stock_item_id,
        reservation_id
    );

CREATE INDEX stock_ledger_entries_stock_item_created_idx
    ON inventory.stock_ledger_entries (store_id,
        stock_item_id,
        created_at DESC,
        id DESC
    );

ALTER TABLE inventory.inventory_locations ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.stock_items ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.inventory_reservations ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.inventory_reservation_lines ENABLE ROW LEVEL SECURITY;

ALTER TABLE inventory.stock_ledger_entries ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON inventory.inventory_locations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.stock_items
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.inventory_reservations
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.inventory_reservation_lines
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON inventory.stock_ledger_entries
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA inventory TO chaos_runtime;

REVOKE UPDATE, DELETE
    ON inventory.stock_ledger_entries FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA inventory TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA inventory
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;
