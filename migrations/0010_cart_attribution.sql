-- Store ad-platform attribution captured at checkout creation directly on the
-- Cart, namespaced by platform (e.g. "meta") so a future platform is an
-- additive JSON key, not a schema change. This replaces correlating a
-- separate browser-recorded analytics event at payment confirmation: the
-- Cart row already serializes checkout creation, so attribution capture is
-- atomic with it.

ALTER TABLE commerce.carts
    ADD COLUMN attribution JSONB;

ALTER TABLE commerce.carts
    ADD CONSTRAINT carts_attribution_check
        CHECK (
            attribution IS NULL
            OR (jsonb_typeof(attribution) = 'object' AND pg_column_size(attribution) <= 4096)
        );
