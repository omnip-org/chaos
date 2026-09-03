-- Snapshot one presentation image URL onto each Order line.
--
-- Order lines are an immutable purchase-time snapshot (title, SKU, price are all
-- stored as literal values, not catalog references). The Worker builds the
-- Stripe Checkout Session from commerce.order_lines alone, so the image the
-- shopper saw at checkout is captured here rather than resolved live from the
-- mutable catalog. The URL is the ready Media asset's public URL resolved with
-- the exact Variant -> Option Value -> Product fallback at order creation.

ALTER TABLE commerce.order_lines
    ADD COLUMN image_url TEXT;

ALTER TABLE commerce.order_lines
    ADD CONSTRAINT order_lines_image_url_check
        CHECK (image_url IS NULL OR (length(image_url) BETWEEN 9 AND 2048 AND image_url ~ '^https://'));
