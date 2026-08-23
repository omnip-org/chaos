CREATE TYPE commerce.product_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE commerce.variant_status AS ENUM ('active', 'archived');

CREATE TYPE commerce.collection_status AS ENUM ('draft', 'active', 'archived');

CREATE TYPE commerce.media_kind AS ENUM ('image', 'video');

CREATE TYPE commerce.media_asset_status AS ENUM ('pending_upload', 'ready', 'archived');

CREATE TYPE commerce.review_status AS ENUM ('pending', 'approved', 'rejected');

CREATE TABLE commerce.products (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               commerce.product_status    NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT products_handle_format_check CHECK (
        handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'
    ),
    CONSTRAINT products_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT products_description_length_check CHECK (
        length(description) <= 100000
    ),
    CONSTRAINT products_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.product_options (
    id                   UUID                 NOT NULL PRIMARY KEY,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    name                 extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, product_id, name),
    UNIQUE (store_id, product_id, position),
    UNIQUE (store_id, product_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_options_name_length_check CHECK (
        length(trim(name::text)) BETWEEN 1 AND 80
    ),
    CONSTRAINT product_options_position_check CHECK (
        position BETWEEN 0 AND 9
    )
);

CREATE TABLE commerce.product_option_values (
    id                   UUID                 NOT NULL PRIMARY KEY,
    store_id             UUID                 NOT NULL,
    product_id           UUID                 NOT NULL,
    option_id            UUID                 NOT NULL,
    value                extensions.citext    NOT NULL,
    position             SMALLINT             NOT NULL,
    created_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, product_id, option_id, value),
    UNIQUE (store_id, product_id, option_id, position),
    UNIQUE (store_id, product_id, option_id, id),
    FOREIGN KEY (store_id, product_id, option_id)
        REFERENCES commerce.product_options(store_id, product_id, id)
        ON DELETE CASCADE,
    CONSTRAINT product_option_values_value_length_check CHECK (
        length(trim(value::text)) BETWEEN 1 AND 120
    ),
    CONSTRAINT product_option_values_position_check CHECK (
        position BETWEEN 0 AND 999
    )
);

CREATE TABLE commerce.product_variants (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    product_id           UUID                       NOT NULL,
    title                TEXT                       NOT NULL,
    sku                  extensions.citext,
    status               commerce.variant_status    NOT NULL DEFAULT 'active',
    requires_shipping    BOOLEAN                    NOT NULL DEFAULT true,
    track_inventory      BOOLEAN                    NOT NULL DEFAULT true,
    on_hand_quantity     BIGINT                     NOT NULL DEFAULT 0,
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    UNIQUE (store_id, product_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT product_variants_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT product_variants_sku_length_check CHECK (
        sku IS NULL OR length(trim(sku::text)) BETWEEN 1 AND 64
    ),
    CONSTRAINT product_variants_sku_characters_check CHECK (
        sku IS NULL OR sku::text !~ '[[:cntrl:]]'
    ),
    CONSTRAINT product_variants_on_hand_nonnegative_check CHECK (
        on_hand_quantity >= 0
    ),
    CONSTRAINT product_variants_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.variant_selected_options (
    store_id             UUID    NOT NULL,
    product_id           UUID    NOT NULL,
    variant_id           UUID    NOT NULL,
    option_id            UUID    NOT NULL,
    option_value_id      UUID    NOT NULL,

    PRIMARY KEY (store_id, variant_id, option_id),
    UNIQUE (store_id, variant_id, option_value_id),
    FOREIGN KEY (store_id, product_id, variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, option_id, option_value_id)
        REFERENCES commerce.product_option_values(store_id,
            product_id,
            option_id,
            id
        ) ON DELETE CASCADE
);

CREATE TABLE commerce.product_publications (
    store_id             UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id, sales_channel_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.collections (
    id                   UUID                       NOT NULL PRIMARY KEY,
    store_id             UUID                       NOT NULL,
    handle               extensions.citext          NOT NULL,
    title                TEXT                       NOT NULL,
    description          TEXT                       NOT NULL DEFAULT '',
    status               commerce.collection_status  NOT NULL DEFAULT 'draft',
    metadata             JSONB,
    created_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, handle),
    UNIQUE (store_id, id),
    FOREIGN KEY (store_id)
        REFERENCES commerce.stores(id) ON DELETE CASCADE,
    CONSTRAINT collections_handle_format_check CHECK (
        handle::text ~ '^[a-z0-9][a-z0-9-]{0,126}[a-z0-9]$'
    ),
    CONSTRAINT collections_title_length_check CHECK (
        length(trim(title)) BETWEEN 1 AND 255
    ),
    CONSTRAINT collections_description_length_check CHECK (
        length(description) <= 100000
    ),
    CONSTRAINT collections_metadata_size_check CHECK (
        metadata IS NULL OR octet_length(metadata::text) <= 32768
    )
);

CREATE TABLE commerce.collection_products (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    product_id           UUID        NOT NULL,
    position             INTEGER     NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, collection_id, product_id),
    UNIQUE (store_id, collection_id, position),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_products_position_check CHECK (position BETWEEN 0 AND 999)
);

CREATE TABLE commerce.collection_publications (
    store_id             UUID        NOT NULL,
    collection_id        UUID        NOT NULL,
    sales_channel_id     UUID        NOT NULL,
    published_at         TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, collection_id, sales_channel_id),
    FOREIGN KEY (store_id, collection_id)
        REFERENCES commerce.collections(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES commerce.store_sales_channels(id) ON DELETE CASCADE
);

CREATE TABLE commerce.media_assets (
    id                   UUID                        NOT NULL PRIMARY KEY,
    store_id             UUID                        NOT NULL,
    product_id           UUID                        NOT NULL,
    product_variant_id   UUID,
    object_key           TEXT                        NOT NULL UNIQUE,
    file_name            TEXT                        NOT NULL,
    media_type           TEXT                        NOT NULL,
    media_kind           commerce.media_kind          NOT NULL,
    byte_size            BIGINT                      NOT NULL,
    sha256_digest        BYTEA                       NOT NULL,
    alt_text             TEXT                        NOT NULL DEFAULT '',
    position             SMALLINT                    NOT NULL,
    status               commerce.media_asset_status  NOT NULL DEFAULT 'pending_upload',
    public_url           TEXT,
    created_by           UUID                        NOT NULL,
    ready_by             UUID,
    archived_by          UUID,
    ready_at             TIMESTAMPTZ,
    archived_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                 NOT NULL,
    updated_at           TIMESTAMPTZ                 NOT NULL,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, product_id, product_variant_id)
        REFERENCES commerce.product_variants(store_id, product_id, id),
    FOREIGN KEY (created_by) REFERENCES identity.users(id),
    FOREIGN KEY (ready_by) REFERENCES identity.users(id),
    FOREIGN KEY (archived_by) REFERENCES identity.users(id),
    CONSTRAINT media_assets_object_key_check CHECK (
        length(object_key) BETWEEN 20 AND 255
        AND object_key ~ '^stores/[0-9a-f-]{36}/media/[0-9a-f-]{36}/original$'
    ),
    CONSTRAINT media_assets_file_name_check CHECK (
        length(trim(file_name)) BETWEEN 1 AND 255
        AND file_name !~ '[[:cntrl:]/\\]'
    ),
    CONSTRAINT media_assets_type_kind_check CHECK (
        (media_kind = 'image' AND media_type IN (
            'image/jpeg', 'image/png', 'image/webp', 'image/avif', 'image/gif'
        ) AND byte_size BETWEEN 1 AND 26214400)
        OR (media_kind = 'video' AND media_type IN (
            'video/mp4', 'video/webm'
        ) AND byte_size BETWEEN 1 AND 524288000)
    ),
    CONSTRAINT media_assets_sha256_check CHECK (octet_length(sha256_digest) = 32),
    CONSTRAINT media_assets_alt_text_check CHECK (
        length(alt_text) <= 500 AND alt_text !~ '[[:cntrl:]]'
    ),
    CONSTRAINT media_assets_position_check CHECK (position BETWEEN 0 AND 99),
    CONSTRAINT media_assets_public_url_check CHECK (
        public_url IS NULL OR (length(public_url) BETWEEN 12 AND 2048 AND public_url ~ '^https://')
    ),
    CONSTRAINT media_assets_lifecycle_check CHECK (
        (status = 'pending_upload' AND public_url IS NULL
            AND ready_by IS NULL AND ready_at IS NULL
            AND archived_by IS NULL AND archived_at IS NULL)
        OR (status = 'ready' AND public_url IS NOT NULL
            AND ready_by IS NOT NULL AND ready_at IS NOT NULL
            AND archived_by IS NULL AND archived_at IS NULL)
        OR (status = 'archived' AND archived_by IS NOT NULL AND archived_at IS NOT NULL
            AND ((public_url IS NULL AND ready_by IS NULL AND ready_at IS NULL)
                OR (public_url IS NOT NULL AND ready_by IS NOT NULL AND ready_at IS NOT NULL)))
    )
);

CREATE TABLE commerce.reviews (
    id                   UUID                     NOT NULL PRIMARY KEY,
    store_id             UUID                     NOT NULL,
    product_id           UUID                     NOT NULL,
    parent_review_id     UUID,
    rating               SMALLINT,
    title                TEXT,
    content              TEXT                     NOT NULL,
    author_name          TEXT                     NOT NULL,
    author_email         extensions.citext,
    status               commerce.review_status   NOT NULL DEFAULT 'pending',
    is_staff_reply       BOOLEAN                  NOT NULL DEFAULT false,
    verified_buyer       BOOLEAN                  NOT NULL DEFAULT false,
    approved_by_user_id  UUID,
    approved_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (store_id, parent_review_id)
        REFERENCES commerce.reviews(store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (approved_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT reviews_rating_shape_check CHECK (
        (is_staff_reply AND rating IS NULL AND parent_review_id IS NOT NULL)
        OR (NOT is_staff_reply AND rating IS NOT NULL AND rating BETWEEN 1 AND 5
            AND parent_review_id IS NULL)
    ),
    CONSTRAINT reviews_content_length_check CHECK (
        length(content) BETWEEN 1 AND 10000
    ),
    CONSTRAINT reviews_title_length_check CHECK (
        title IS NULL OR length(title) <= 255
    ),
    CONSTRAINT reviews_author_name_length_check CHECK (
        length(trim(author_name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT reviews_approval_shape_check CHECK (
        (status = 'approved') = (approved_at IS NOT NULL AND approved_by_user_id IS NOT NULL)
    ),
    CONSTRAINT reviews_verified_buyer_requires_approval_check CHECK (
        NOT verified_buyer OR status = 'approved'
    )
);

CREATE INDEX products_store_status_created_idx
    ON commerce.products (store_id, status, created_at DESC, id DESC);

CREATE UNIQUE INDEX product_variants_store_sku_key
    ON commerce.product_variants (store_id, sku)
    WHERE sku IS NOT NULL;

CREATE INDEX product_variants_product_status_idx
    ON commerce.product_variants (store_id, product_id, status);

CREATE INDEX product_publications_channel_product_idx
    ON commerce.product_publications (store_id,
        sales_channel_id,
        product_id
    );

CREATE INDEX collections_store_status_created_idx
    ON commerce.collections (store_id, status, created_at DESC, id DESC
    );

CREATE INDEX collection_products_product_idx
    ON commerce.collection_products (store_id, product_id, collection_id);

CREATE INDEX collection_publications_channel_collection_idx
    ON commerce.collection_publications (store_id, sales_channel_id, collection_id
    );

CREATE UNIQUE INDEX media_assets_product_position_active_idx
    ON commerce.media_assets (store_id, product_id, position)
    WHERE status <> 'archived';

CREATE INDEX media_assets_product_status_position_idx
    ON commerce.media_assets (store_id, product_id, status, position, id
    );

CREATE INDEX reviews_product_status_idx
    ON commerce.reviews (store_id, product_id, status, created_at, id);

CREATE INDEX reviews_parent_idx
    ON commerce.reviews (store_id, parent_review_id)
    WHERE parent_review_id IS NOT NULL;

ALTER TABLE commerce.products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_option_values ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_variants ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.variant_selected_options ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.product_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collections ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_products ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.collection_publications ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.media_assets ENABLE ROW LEVEL SECURITY;

ALTER TABLE commerce.reviews ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );
CREATE POLICY store_isolation ON commerce.product_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_option_values
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_variants
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.variant_selected_options
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.product_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collections
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collection_products
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.collection_publications
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.media_assets
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON commerce.reviews
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE TABLE commerce.product_documents (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    document            TSVECTOR    NOT NULL,
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES commerce.products(store_id, id) ON DELETE CASCADE
);

CREATE INDEX product_documents_search_idx
    ON commerce.product_documents USING GIN (document);

CREATE FUNCTION commerce.refresh_product_document(UUID, UUID)
RETURNS VOID LANGUAGE SQL SECURITY DEFINER SET search_path = pg_catalog AS $$
    INSERT INTO commerce.product_documents (store_id, product_id, document, indexed_at)
    SELECT product.store_id, product.id,
           to_tsvector('simple', concat_ws(
               ' ', product.handle::text, product.title, product.description,
               string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
           )), CURRENT_TIMESTAMP
      FROM commerce.products AS product
      LEFT JOIN commerce.product_variants AS variant
        ON variant.store_id = product.store_id AND variant.product_id = product.id
     WHERE product.store_id = $1 AND product.id = $2
     GROUP BY product.store_id, product.id
    ON CONFLICT (store_id, product_id) DO UPDATE
        SET document = EXCLUDED.document, indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION commerce.capture_product_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO integration.event_outbox (
        id, store_id, aggregate_type, aggregate_id, event_type, payload
    ) VALUES (
        uuidv7(), NEW.store_id, 'product', NEW.id,
        'search.product.changed', jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.capture_variant_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    owning_store_id UUID;
    changed_product_id UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        owning_store_id := OLD.store_id;
        changed_product_id := OLD.product_id;
    ELSE
        owning_store_id := NEW.store_id;
        changed_product_id := NEW.product_id;
    END IF;
    IF EXISTS (
        SELECT 1 FROM commerce.stores
         WHERE id = owning_store_id
    ) THEN
        INSERT INTO integration.event_outbox (
            id, store_id, aggregate_type, aggregate_id, event_type, payload
        ) VALUES (
            uuidv7(), owning_store_id, 'product', changed_product_id,
            'search.product.changed', jsonb_build_object('product_id', changed_product_id)
        );
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION commerce.rebuild_store_products(UUID)
RETURNS BIGINT LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE product_id UUID; rebuilt BIGINT := 0;
BEGIN
    DELETE FROM commerce.product_documents WHERE store_id = $1;
    FOR product_id IN SELECT id FROM commerce.products
        WHERE store_id = $1
    LOOP
        PERFORM commerce.refresh_product_document($1, product_id);
        rebuilt := rebuilt + 1;
    END LOOP;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION commerce.process_events(INTEGER, TIMESTAMPTZ)
RETURNS BIGINT LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE event RECORD; processed BIGINT := 0;
BEGIN
    FOR event IN
        SELECT outbox.id,
               outbox.store_id,
               outbox.aggregate_id,
               outbox.attempts
          FROM integration.claim_routed_event_outbox(
                   'chaos_search_events', $1
               ) AS outbox
    LOOP
        BEGIN
            PERFORM commerce.refresh_product_document(
                event.store_id, event.aggregate_id
            );
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, true, '', 8, $2
            );
            processed := processed + 1;
        EXCEPTION WHEN OTHERS THEN
            PERFORM integration.finish_event_outbox(
                event.id, event.attempts, false, SQLERRM, 8, $2
            );
        END;
    END LOOP;
    RETURN processed;
END;
$$;

CREATE TRIGGER products_search_change
AFTER INSERT OR UPDATE OF handle, title, description ON commerce.products
FOR EACH ROW EXECUTE FUNCTION commerce.capture_product_change();

CREATE TRIGGER variants_search_change
AFTER INSERT OR UPDATE OF title, sku OR DELETE ON commerce.product_variants
FOR EACH ROW EXECUTE FUNCTION commerce.capture_variant_change();

ALTER TABLE commerce.product_documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON commerce.product_documents
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );
