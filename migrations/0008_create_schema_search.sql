CREATE SCHEMA search;

COMMENT ON SCHEMA search IS
    'Rebuildable Store-isolated read models for storefront discovery';

CREATE TABLE search.product_documents (
    store_id            UUID        NOT NULL,
    product_id          UUID        NOT NULL,
    document            TSVECTOR    NOT NULL,
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, product_id),
    FOREIGN KEY (store_id, product_id)
        REFERENCES catalog.products(store_id, id) ON DELETE CASCADE
);

CREATE INDEX product_documents_search_idx
    ON search.product_documents USING GIN (document);

CREATE FUNCTION search.refresh_product_document(UUID, UUID)
RETURNS VOID LANGUAGE SQL SECURITY DEFINER SET search_path = pg_catalog AS $$
    INSERT INTO search.product_documents (store_id, product_id, document, indexed_at)
    SELECT product.store_id, product.id,
           to_tsvector('simple', concat_ws(
               ' ', product.handle::text, product.title, product.description,
               string_agg(concat_ws(' ', variant.title, variant.sku::text), ' ')
           )), CURRENT_TIMESTAMP
      FROM catalog.products AS product
      LEFT JOIN catalog.product_variants AS variant
        ON variant.store_id = product.store_id AND variant.product_id = product.id
     WHERE product.store_id = $1 AND product.id = $2
     GROUP BY product.store_id, product.id
    ON CONFLICT (store_id, product_id) DO UPDATE
        SET document = EXCLUDED.document, indexed_at = EXCLUDED.indexed_at;
$$;

CREATE FUNCTION search.capture_product_change()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO integration.outbox_events (
        id, store_id, aggregate_type, aggregate_id, event_type, payload
    ) VALUES (
        uuidv7(), NEW.store_id, 'product', NEW.id,
        'search.product.changed', jsonb_build_object('product_id', NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE FUNCTION search.capture_variant_change()
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
        SELECT 1 FROM merchant.stores
         WHERE id = owning_store_id
    ) THEN
        INSERT INTO integration.outbox_events (
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

CREATE FUNCTION search.rebuild_store_products(UUID)
RETURNS BIGINT LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE product_id UUID; rebuilt BIGINT := 0;
BEGIN
    DELETE FROM search.product_documents WHERE store_id = $1;
    FOR product_id IN SELECT id FROM catalog.products
        WHERE store_id = $1
    LOOP
        PERFORM search.refresh_product_document($1, product_id);
        rebuilt := rebuilt + 1;
    END LOOP;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION search.process_events(UUID, INTEGER, TIMESTAMPTZ)
RETURNS BIGINT LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE event RECORD; processed BIGINT := 0;
BEGIN
    FOR event IN
        SELECT outbox.id, outbox.store_id, outbox.aggregate_id
          FROM integration.outbox_events AS outbox
          INNER JOIN integration.event_consumer_registry AS registry
            ON registry.event_type = outbox.event_type
           AND registry.consumer_owner = 'search.product_indexer'
         WHERE outbox.status = 'pending' AND outbox.event_type = 'search.product.changed'
           AND outbox.available_at <= $3
         ORDER BY outbox.available_at, outbox.created_at, outbox.id
         FOR UPDATE OF outbox SKIP LOCKED
         LIMIT greatest(least($2, 100), 1)
    LOOP
        UPDATE integration.outbox_events
           SET status = 'processing', attempts = attempts + 1,
               locked_by = $1, locked_at = $3
         WHERE id = event.id;
        PERFORM search.refresh_product_document(
            event.store_id, event.aggregate_id
        );
        UPDATE integration.outbox_events
           SET status = 'processed', processed_at = $3,
               locked_by = NULL, locked_at = NULL
         WHERE id = event.id AND locked_by = $1;
        processed := processed + 1;
    END LOOP;
    RETURN processed;
END;
$$;

CREATE TRIGGER products_search_change
AFTER INSERT OR UPDATE OF handle, title, description ON catalog.products
FOR EACH ROW EXECUTE FUNCTION search.capture_product_change();

CREATE TRIGGER variants_search_change
AFTER INSERT OR UPDATE OF title, sku OR DELETE ON catalog.product_variants
FOR EACH ROW EXECUTE FUNCTION search.capture_variant_change();

ALTER TABLE search.product_documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON search.product_documents
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

GRANT SELECT ON ALL TABLES IN SCHEMA search TO chaos_runtime;

GRANT EXECUTE ON FUNCTION search.rebuild_store_products(UUID) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION search.process_events(UUID, INTEGER, TIMESTAMPTZ) TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA search
    GRANT SELECT ON TABLES TO chaos_runtime;

GRANT USAGE ON SCHEMA search TO chaos_runtime;
