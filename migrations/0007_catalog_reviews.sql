CREATE TYPE catalog.review_status AS ENUM ('pending', 'approved', 'rejected');
CREATE TYPE catalog.review_event_kind AS ENUM ('submitted', 'approved', 'rejected', 'reply_added');

CREATE TABLE catalog.reviews (
    id                   UUID                     NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                     NOT NULL,
    store_id             UUID                     NOT NULL,
    product_id           UUID                     NOT NULL,
    parent_review_id     UUID,
    rating               SMALLINT,
    title                TEXT,
    content              TEXT                     NOT NULL,
    author_name          TEXT                     NOT NULL,
    author_email         extensions.citext,
    status               catalog.review_status    NOT NULL DEFAULT 'pending',
    is_staff_reply       BOOLEAN                  NOT NULL DEFAULT false,
    verified_buyer       BOOLEAN                  NOT NULL DEFAULT false,
    approved_by_user_id  UUID,
    approved_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (merchant_account_id, store_id, id),
    FOREIGN KEY (merchant_account_id, store_id, product_id)
        REFERENCES catalog.products(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (merchant_account_id, store_id, parent_review_id)
        REFERENCES catalog.reviews(merchant_account_id, store_id, id) ON DELETE CASCADE,
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

CREATE INDEX reviews_product_status_idx
    ON catalog.reviews (merchant_account_id, store_id, product_id, status, created_at, id);
CREATE INDEX reviews_parent_idx
    ON catalog.reviews (merchant_account_id, store_id, parent_review_id)
    WHERE parent_review_id IS NOT NULL;

CREATE TABLE catalog.review_events (
    id                   UUID                        NOT NULL PRIMARY KEY,
    merchant_account_id  UUID                        NOT NULL,
    store_id             UUID                        NOT NULL,
    review_id            UUID                        NOT NULL,
    event_kind           catalog.review_event_kind   NOT NULL,
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (merchant_account_id, store_id, review_id)
        REFERENCES catalog.reviews(merchant_account_id, store_id, id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT review_events_actor_shape_check CHECK (
        (event_kind = 'submitted' AND actor_user_id IS NULL)
        OR (event_kind IN ('approved', 'rejected', 'reply_added') AND actor_user_id IS NOT NULL)
    )
);

CREATE INDEX review_events_review_occurred_idx
    ON catalog.review_events (merchant_account_id, store_id, review_id, occurred_at, id);

ALTER TABLE catalog.reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.review_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY merchant_account_isolation ON catalog.reviews
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

CREATE POLICY merchant_account_isolation ON catalog.review_events
    USING (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    )
    WITH CHECK (
        merchant_account_id =
        nullif(current_setting('app.merchant_account_id', true), '')::uuid
    );

REVOKE DELETE ON catalog.reviews FROM chaos_runtime;
REVOKE UPDATE, DELETE ON catalog.review_events FROM chaos_runtime;

ALTER TYPE merchant.api_key_scope ADD VALUE 'reviews:write';
