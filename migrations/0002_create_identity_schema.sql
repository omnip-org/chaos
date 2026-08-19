CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');

CREATE TYPE merchant.store_role AS ENUM ('owner', 'member');

CREATE TYPE merchant.store_status AS ENUM ('active', 'inactive');

CREATE TYPE merchant.sales_channel_kind AS ENUM (
    'web',
    'mobile',
    'point_of_sale',
    'marketplace',
    'custom'
);

CREATE TYPE merchant.sales_channel_status AS ENUM ('active', 'archived');

CREATE TYPE merchant.store_locale_event_kind AS ENUM ('enabled', 'disabled', 'default_changed');

CREATE TYPE merchant.api_key_class AS ENUM ('publishable', 'secret');

CREATE TYPE merchant.api_key_scope AS ENUM (
    'analytics:write',
    'catalog:read',
    'carts:write',
    'checkout:write',
    'orders:read',
    'customers:write',
    'mcp:tools',
    'products:read',
    'products:write',
    'pricing:read',
    'pricing:write',
    'inventory:read',
    'inventory:write',
    'collections:read',
    'collections:write',
    'orders:write',
    'fulfillment:read',
    'fulfillment:write',
    'store_admin:read',
    'store_admin:write',
    'payments:write',
    'media:read',
    'media:write',
    'reviews:write',
    'api_keys:read',
    'api_keys:write',
    'provider_secrets:write'
);

CREATE TYPE integration.idempotency_scope AS ENUM ('user', 'store', 'shopper');

CREATE TYPE integration.queue_status AS ENUM ('pending', 'processing', 'processed', 'dead_letter');

CREATE TABLE identity.users (
    id          UUID                    NOT NULL PRIMARY KEY,
    email       extensions.citext       NOT NULL UNIQUE,
    status      identity.user_status    NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT users_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    )
);

CREATE TABLE identity.magic_link_challenges (
    id            UUID                 NOT NULL PRIMARY KEY,
    email         extensions.citext    NOT NULL,
    token_digest  BYTEA                NOT NULL UNIQUE,
    expires_at    TIMESTAMPTZ          NOT NULL,
    consumed_at   TIMESTAMPTZ,
    created_at    TIMESTAMPTZ          NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT magic_link_challenges_token_digest_length_check CHECK (
        octet_length(token_digest) = 32
    ),
    CONSTRAINT magic_link_challenges_expiration_check CHECK (
        expires_at > created_at
    )
);

CREATE TABLE identity.sessions (
    id            UUID           NOT NULL PRIMARY KEY,
    user_id       UUID           NOT NULL,
    token_digest  BYTEA          NOT NULL UNIQUE,
    expires_at    TIMESTAMPTZ    NOT NULL,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at  TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT sessions_token_digest_length_check CHECK (
        octet_length(token_digest) = 32
    ),
    CONSTRAINT sessions_expiration_check CHECK (
        expires_at > created_at
    )
);

CREATE TABLE identity.passkey_credentials (
    id             UUID           NOT NULL PRIMARY KEY,
    user_id        UUID           NOT NULL,
    credential_id  BYTEA          NOT NULL UNIQUE,
    credential     JSONB          NOT NULL,
    name           TEXT           NOT NULL,
    created_at     TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TIMESTAMPTZ    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at   TIMESTAMPTZ,

    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT passkey_credentials_credential_id_length_check CHECK (
        octet_length(credential_id) BETWEEN 16 AND 1024
    ),
    CONSTRAINT passkey_credentials_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    )
);

CREATE TABLE merchant.stores (
    id                   UUID                     NOT NULL PRIMARY KEY,
    code                 extensions.citext        NOT NULL UNIQUE,
    name                 TEXT                     NOT NULL,
    default_region       CHAR(2)                  NOT NULL DEFAULT 'US',
    default_currency     CHAR(3)                  NOT NULL DEFAULT 'USD',
    default_locale       VARCHAR(63)              NOT NULL DEFAULT 'en-US',
    status               merchant.store_status    NOT NULL DEFAULT 'inactive',
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT stores_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT stores_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    ),
    CONSTRAINT stores_region_format_check CHECK (
        default_region ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT stores_currency_format_check CHECK (
        default_currency ~ '^[A-Z]{3}$'
    ),
    CONSTRAINT stores_default_locale_check CHECK (
        default_locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE merchant.store_memberships (
    store_id    UUID                    NOT NULL,
    user_id     UUID                    NOT NULL,
    role        merchant.store_role     NOT NULL,
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (store_id, user_id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE
);

CREATE TABLE merchant.store_locales (
    store_id            UUID        NOT NULL,
    locale              VARCHAR(63) NOT NULL,
    created_by_user_id  UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (store_id, locale),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (created_by_user_id) REFERENCES identity.users(id),
    CONSTRAINT store_locales_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
    )
);

CREATE TABLE merchant.store_locale_events (
    id                  UUID                             NOT NULL PRIMARY KEY,
    store_id            UUID                             NOT NULL,
    locale              VARCHAR(63)                      NOT NULL,
    previous_locale     VARCHAR(63),
    event_kind          merchant.store_locale_event_kind NOT NULL,
    actor_user_id       UUID                             NOT NULL,
    occurred_at         TIMESTAMPTZ                      NOT NULL,

    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (actor_user_id) REFERENCES identity.users(id),
    CONSTRAINT store_locale_events_locale_check CHECK (
        locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$'
        AND (previous_locale IS NULL
            OR previous_locale ~ '^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$')
    ),
    CONSTRAINT store_locale_events_shape_check CHECK (
        (event_kind = 'default_changed' AND previous_locale IS NOT NULL)
        OR (event_kind <> 'default_changed' AND previous_locale IS NULL)
    )
);

CREATE TABLE merchant.sales_channels (
    id                   UUID                              NOT NULL PRIMARY KEY,
    store_id             UUID                              NOT NULL,
    code                 extensions.citext                 NOT NULL,
    name                 TEXT                              NOT NULL,
    kind                 merchant.sales_channel_kind       NOT NULL,
    status               merchant.sales_channel_status     NOT NULL DEFAULT 'active',
    is_default           BOOLEAN                           NOT NULL DEFAULT false,
    created_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                       NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (store_id, code),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT sales_channels_code_format_check CHECK (
        code::text ~ '^[a-z0-9][a-z0-9-]{0,30}[a-z0-9]$'
    ),
    CONSTRAINT sales_channels_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 120
    )
);

CREATE TABLE merchant.store_currencies (
    store_id             UUID       NOT NULL,
    currency             CHAR(3)    NOT NULL,
    enabled              BOOLEAN    NOT NULL DEFAULT true,

    PRIMARY KEY (store_id, currency),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    CONSTRAINT store_currencies_currency_format_check CHECK (
        currency ~ '^[A-Z]{3}$'
    )
);

CREATE TABLE merchant.api_keys (
    id                   UUID                      NOT NULL PRIMARY KEY,
    store_id             UUID                      NOT NULL,
    sales_channel_id     UUID,
    key_identifier       TEXT                      NOT NULL UNIQUE,
    secret_digest        BYTEA                     NOT NULL,
    display_suffix       CHAR(4)                   NOT NULL,
    name                 TEXT                      NOT NULL,
    class                merchant.api_key_class    NOT NULL,
    created_by_user_id   UUID                      NOT NULL,
    revoked_by_user_id   UUID,
    expires_at           TIMESTAMPTZ,
    last_used_at         TIMESTAMPTZ,
    revoked_at           TIMESTAMPTZ,
    created_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ               NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (sales_channel_id)
        REFERENCES merchant.sales_channels(id),
    FOREIGN KEY (created_by_user_id)
        REFERENCES identity.users(id),
    FOREIGN KEY (revoked_by_user_id)
        REFERENCES identity.users(id),
    CONSTRAINT api_keys_identifier_format_check CHECK (
        key_identifier ~ '^[A-Za-z0-9_-]{16}$'
    ),
    CONSTRAINT api_keys_secret_digest_length_check CHECK (
        octet_length(secret_digest) = 32
    ),
    CONSTRAINT api_keys_display_suffix_format_check CHECK (
        display_suffix ~ '^[A-Za-z0-9_-]{4}$'
    ),
    CONSTRAINT api_keys_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    ),
    CONSTRAINT api_keys_expiration_check CHECK (
        expires_at IS NULL OR expires_at > created_at
    ),
    CONSTRAINT api_keys_revocation_check CHECK (
        (revoked_at IS NULL AND revoked_by_user_id IS NULL)
        OR (revoked_at IS NOT NULL AND revoked_by_user_id IS NOT NULL)
    )
);

CREATE TABLE merchant.api_key_scopes (
    api_key_id           UUID                      NOT NULL,
    scope                merchant.api_key_scope    NOT NULL,

    PRIMARY KEY (api_key_id, scope),
    FOREIGN KEY (api_key_id)
        REFERENCES merchant.api_keys(id) ON DELETE CASCADE
);

CREATE TABLE integration.idempotency_records (
    id                   UUID                             NOT NULL PRIMARY KEY,
    scope                integration.idempotency_scope    NOT NULL,
    scope_id             UUID                             NOT NULL,
    operation            TEXT                             NOT NULL,
    idempotency_key      TEXT                             NOT NULL,
    request_fingerprint  BYTEA                            NOT NULL,
    response_status      SMALLINT,
    response_body        JSONB,
    completed_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMPTZ                      NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (scope, scope_id, operation, idempotency_key),
    CONSTRAINT idempotency_records_operation_length_check CHECK (
        length(operation) BETWEEN 1 AND 120
    ),
    CONSTRAINT idempotency_records_key_length_check CHECK (
        octet_length(idempotency_key) BETWEEN 1 AND 255
    ),
    CONSTRAINT idempotency_records_request_fingerprint_length_check CHECK (
        octet_length(request_fingerprint) = 32
    ),
    CONSTRAINT idempotency_records_response_completion_check CHECK (
        (response_status IS NULL AND response_body IS NULL AND completed_at IS NULL)
        OR
        (response_status BETWEEN 200 AND 599 AND response_body IS NOT NULL AND completed_at IS NOT NULL)
    )
);

CREATE TABLE integration.webhook_inbox (
    id                         UUID                     NOT NULL PRIMARY KEY,
    store_id                   UUID                     NOT NULL,
    provider                   TEXT                     NOT NULL,
    provider_event_id          TEXT                     NOT NULL,
    event_type                 TEXT                     NOT NULL,
    external_account_reference TEXT                     NOT NULL,
    payload                    JSONB                    NOT NULL,
    status                     integration.queue_status NOT NULL DEFAULT 'pending',
    attempts                   INTEGER                  NOT NULL DEFAULT 0,
    available_at               TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by                  UUID,
    locked_at                  TIMESTAMPTZ,
    processed_at               TIMESTAMPTZ,
    last_error                 TEXT,
    verified_at                TIMESTAMPTZ              NOT NULL,
    created_at                 TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (provider, provider_event_id),
    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id),
    CONSTRAINT webhook_inbox_attempts_nonnegative_check CHECK (attempts >= 0),
    CONSTRAINT webhook_inbox_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT webhook_inbox_processed_shape_check CHECK (
        (status = 'processed' AND processed_at IS NOT NULL)
        OR (status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT webhook_inbox_lease_shape_check CHECK (
        (status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    )
);

CREATE TABLE integration.event_consumer_registry (
    event_type      TEXT PRIMARY KEY,
    consumer_owner  TEXT,
    description     TEXT NOT NULL,

    CONSTRAINT event_consumer_registry_event_type_check CHECK (
        event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT event_consumer_registry_owner_check CHECK (
        consumer_owner IS NULL
        OR consumer_owner ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    CONSTRAINT event_consumer_registry_description_check CHECK (
        length(trim(description)) BETWEEN 1 AND 255
    )
);

CREATE TABLE integration.outbox_events (
    id                   UUID                     NOT NULL PRIMARY KEY,
    store_id             UUID                     NOT NULL,
    aggregate_type       TEXT                     NOT NULL,
    aggregate_id         UUID                     NOT NULL,
    event_type           TEXT                     NOT NULL,
    payload              JSONB                    NOT NULL,
    status               integration.queue_status NOT NULL DEFAULT 'pending',
    attempts             INTEGER                  NOT NULL DEFAULT 0,
    available_at         TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_by            UUID,
    locked_at            TIMESTAMPTZ,
    processed_at         TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (store_id)
        REFERENCES merchant.stores(id) ON DELETE CASCADE,
    FOREIGN KEY (event_type)
        REFERENCES integration.event_consumer_registry(event_type),
    CONSTRAINT outbox_events_attempts_nonnegative_check CHECK (attempts >= 0),
    CONSTRAINT outbox_events_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT outbox_events_processed_shape_check CHECK (
        (status = 'processed' AND processed_at IS NOT NULL)
        OR (status <> 'processed' AND processed_at IS NULL)
    ),
    CONSTRAINT outbox_events_lease_shape_check CHECK (
        (status = 'processing' AND locked_by IS NOT NULL AND locked_at IS NOT NULL)
        OR (status <> 'processing' AND locked_by IS NULL AND locked_at IS NULL)
    )
);

CREATE INDEX magic_link_challenges_email_created_idx
    ON identity.magic_link_challenges (email, created_at DESC);

CREATE INDEX sessions_user_expires_idx
    ON identity.sessions (user_id, expires_at DESC);

CREATE INDEX passkey_credentials_user_created_idx
    ON identity.passkey_credentials (user_id, created_at DESC);

CREATE INDEX store_memberships_user_idx
    ON merchant.store_memberships (user_id, store_id);

CREATE INDEX stores_status_idx
    ON merchant.stores (status);

CREATE INDEX store_locale_events_store_occurred_idx
    ON merchant.store_locale_events (store_id, occurred_at, id);

CREATE UNIQUE INDEX sales_channels_one_default_per_store_idx
    ON merchant.sales_channels (store_id)
    WHERE is_default;

CREATE INDEX sales_channels_store_status_idx
    ON merchant.sales_channels (store_id, status);

CREATE INDEX api_keys_store_created_idx
    ON merchant.api_keys (store_id, created_at DESC, id DESC);

CREATE INDEX webhook_inbox_claim_idx
    ON integration.webhook_inbox (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE INDEX outbox_events_claim_idx
    ON integration.outbox_events (status, available_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE FUNCTION merchant.prevent_default_locale_removal()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM merchant.stores
        WHERE id = OLD.store_id
          AND default_locale = OLD.locale
    ) THEN
        RAISE EXCEPTION 'the default Store Locale cannot be disabled'
            USING ERRCODE = '23514';
    END IF;
    RETURN OLD;
END
$$;

CREATE FUNCTION merchant.authenticate_api_key(
    presented_key_identifier  TEXT,
    presented_secret_digest  BYTEA
)
RETURNS TABLE (
    api_key_id           UUID,
    store_id             UUID,
    sales_channel_id     UUID,
    class                TEXT,
    scopes               TEXT[],
    created_by_user_id   UUID
)
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    SELECT api_key.id,
           api_key.store_id,
           sales_channel.id,
           api_key.class::TEXT,
           ARRAY(
               SELECT api_key_scope.scope::TEXT
               FROM merchant.api_key_scopes AS api_key_scope
               WHERE api_key_scope.api_key_id = api_key.id
               ORDER BY api_key_scope.scope::TEXT
           ),
           api_key.created_by_user_id
    FROM merchant.api_keys AS api_key
    INNER JOIN merchant.stores AS store
        ON store.id = api_key.store_id
    LEFT JOIN merchant.sales_channels AS sales_channel
        ON sales_channel.store_id = api_key.store_id
       AND sales_channel.id = COALESCE(
           api_key.sales_channel_id,
           (
               SELECT default_channel.id
               FROM merchant.sales_channels AS default_channel
               WHERE default_channel.store_id = api_key.store_id
                 AND default_channel.is_default
               LIMIT 1
           )
       )
    WHERE api_key.key_identifier = presented_key_identifier
      AND api_key.secret_digest = presented_secret_digest
      AND api_key.revoked_at IS NULL
      AND (api_key.expires_at IS NULL OR api_key.expires_at > CURRENT_TIMESTAMP)
      AND store.status = 'active';
$$;

CREATE FUNCTION integration.queue_metrics()
RETURNS TABLE (pending BIGINT, dead_letter BIGINT, oldest_pending_seconds DOUBLE PRECISION)
LANGUAGE SQL STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT count(*) FILTER (WHERE status = 'pending'),
           count(*) FILTER (WHERE status = 'dead_letter'),
           COALESCE(
               extract(
                   epoch FROM CURRENT_TIMESTAMP -
                       min(created_at) FILTER (WHERE status = 'pending')
               ),
               0
           )
      FROM integration.outbox_events;
$$;

CREATE FUNCTION integration.event_consumer_backlog()
RETURNS TABLE (
    event_type TEXT,
    consumer_owner TEXT,
    pending BIGINT,
    processing BIGINT,
    dead_letter BIGINT,
    processed BIGINT
)
LANGUAGE SQL STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT registry.event_type,
           registry.consumer_owner,
           count(event.id) FILTER (WHERE event.status = 'pending'),
           count(event.id) FILTER (WHERE event.status = 'processing'),
           count(event.id) FILTER (WHERE event.status = 'dead_letter'),
           count(event.id) FILTER (WHERE event.status = 'processed')
      FROM integration.event_consumer_registry AS registry
      LEFT JOIN integration.outbox_events AS event
        ON event.event_type = registry.event_type
     GROUP BY registry.event_type, registry.consumer_owner
     ORDER BY registry.event_type;
$$;

CREATE FUNCTION integration.claim_outbox_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.outbox_events AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
          FROM integration.event_consumer_registry AS registry
         WHERE registry.event_type = event.event_type
           AND registry.consumer_owner = 'payments.provider_dispatch'
           AND event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.outbox_events AS event
        INNER JOIN integration.event_consumer_registry AS registry
          ON registry.event_type = event.event_type
         AND registry.consumer_owner = 'payments.provider_dispatch'
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE OF event SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.outbox_events AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.store_id,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.claim_fulfillment_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.outbox_events AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
          FROM integration.event_consumer_registry AS registry
         WHERE registry.event_type = event.event_type
           AND registry.consumer_owner = 'fulfillment.operations'
           AND event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.outbox_events AS event
        INNER JOIN integration.event_consumer_registry AS registry
          ON registry.event_type = event.event_type
         AND registry.consumer_owner = 'fulfillment.operations'
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE OF event SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.outbox_events AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.store_id,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.claim_webhook_events(
    worker_id UUID,
    batch_size INTEGER,
    claimed_at TIMESTAMPTZ,
    stale_before TIMESTAMPTZ
)
RETURNS TABLE (
    id UUID,
    store_id UUID,
    provider TEXT,
    event_type TEXT,
    payload JSONB,
    attempts INTEGER
)
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    WITH expired AS (
        UPDATE integration.webhook_inbox AS event
           SET status = 'dead_letter',
               locked_by = NULL,
               locked_at = NULL,
               last_error = COALESCE(event.last_error, 'worker lease expired after final attempt')
         WHERE event.status = 'processing' AND event.locked_at <= stale_before
           AND event.attempts >= 8
        RETURNING event.id
    ), claimable AS (
        SELECT event.id
        FROM integration.webhook_inbox AS event
        WHERE (
                (event.status = 'pending' AND event.available_at <= claimed_at)
                OR (event.status = 'processing' AND event.locked_at <= stale_before)
              )
          AND event.attempts < 8
        ORDER BY event.available_at, event.created_at, event.id
        FOR UPDATE SKIP LOCKED
        LIMIT greatest(least(batch_size, 100), 1)
    )
    UPDATE integration.webhook_inbox AS event
       SET status = 'processing',
           attempts = event.attempts + 1,
           locked_by = worker_id,
           locked_at = claimed_at
      FROM claimable
     WHERE event.id = claimable.id
    RETURNING event.id, event.store_id, event.provider,
              event.event_type, event.payload, event.attempts;
$$;

CREATE FUNCTION integration.finish_outbox_event(
    event_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    failure TEXT,
    max_attempts INTEGER,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE integration.outbox_events AS event
       SET status = CASE
               WHEN succeeded THEN 'processed'::integration.queue_status
               WHEN event.attempts >= greatest(max_attempts, 1)
                   THEN 'dead_letter'::integration.queue_status
               ELSE 'pending'::integration.queue_status
           END,
           available_at = CASE
               WHEN succeeded THEN event.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(event.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
           last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
     WHERE event.id = event_id AND event.status = 'processing' AND event.locked_by = worker_id
    RETURNING true;
$$;

CREATE FUNCTION integration.finish_webhook_event(
    event_id UUID,
    worker_id UUID,
    succeeded BOOLEAN,
    failure TEXT,
    max_attempts INTEGER,
    finished_at TIMESTAMPTZ
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
    UPDATE integration.webhook_inbox AS event
       SET status = CASE
               WHEN succeeded THEN 'processed'::integration.queue_status
               WHEN event.attempts >= greatest(max_attempts, 1)
                   THEN 'dead_letter'::integration.queue_status
               ELSE 'pending'::integration.queue_status
           END,
           available_at = CASE
               WHEN succeeded THEN event.available_at
               ELSE finished_at + make_interval(
                   secs => least(power(2, greatest(event.attempts - 1, 0))::integer, 256)
               )
           END,
           locked_by = NULL,
           locked_at = NULL,
           processed_at = CASE WHEN succeeded THEN finished_at ELSE NULL END,
           last_error = CASE WHEN succeeded THEN NULL ELSE left(failure, 2000) END
     WHERE event.id = event_id AND event.status = 'processing' AND event.locked_by = worker_id
    RETURNING true;
$$;

CREATE TRIGGER store_locales_protect_default
BEFORE DELETE ON merchant.store_locales
FOR EACH ROW EXECUTE FUNCTION merchant.prevent_default_locale_removal();

ALTER TABLE merchant.stores ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_memberships ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_locales ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_locale_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.store_currencies ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.sales_channels ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.api_keys ENABLE ROW LEVEL SECURITY;

ALTER TABLE merchant.api_key_scopes ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.idempotency_records ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.webhook_inbox ENABLE ROW LEVEL SECURITY;

ALTER TABLE integration.outbox_events ENABLE ROW LEVEL SECURITY;

CREATE POLICY store_isolation ON merchant.stores
    USING (id = nullif(current_setting('app.store_id', true), '')::uuid)
    WITH CHECK (id = nullif(current_setting('app.store_id', true), '')::uuid);

CREATE POLICY store_directory ON merchant.stores
    FOR SELECT
    USING (
        EXISTS (
            SELECT 1
            FROM merchant.store_memberships AS membership
            WHERE membership.store_id = stores.id
              AND membership.user_id =
                    nullif(current_setting('app.user_id', true), '')::uuid
        )
    );

CREATE POLICY store_isolation ON merchant.store_memberships
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_membership_directory ON merchant.store_memberships
    FOR SELECT
    USING (
        user_id = nullif(current_setting('app.user_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_locales
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_locale_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.store_currencies
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.sales_channels
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.api_keys
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON merchant.api_key_scopes
    USING (
        EXISTS (
            SELECT 1
            FROM merchant.api_keys AS api_key
            WHERE api_key.id = api_key_scopes.api_key_id
              AND api_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    )
    WITH CHECK (
        EXISTS (
            SELECT 1
            FROM merchant.api_keys AS api_key
            WHERE api_key.id = api_key_scopes.api_key_id
              AND api_key.store_id =
                    nullif(current_setting('app.store_id', true), '')::uuid
        )
    );

CREATE POLICY idempotency_scope_isolation ON integration.idempotency_records
    USING (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'store' AND scope_id =
            nullif(current_setting('app.store_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    )
    WITH CHECK (
        (scope = 'user' AND scope_id =
            nullif(current_setting('app.user_id', true), '')::uuid)
        OR
        (scope = 'store' AND scope_id =
            nullif(current_setting('app.store_id', true), '')::uuid)
        OR
        (scope = 'shopper' AND scope_id =
            nullif(current_setting('app.shopper_id', true), '')::uuid)
    );

CREATE POLICY store_isolation ON integration.webhook_inbox
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

CREATE POLICY store_isolation ON integration.outbox_events
    USING (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    )
    WITH CHECK (
        store_id =
        nullif(current_setting('app.store_id', true), '')::uuid
    );

INSERT INTO integration.event_consumer_registry (event_type, consumer_owner, description)
VALUES
    ('payment.create_requested', 'payments.provider_dispatch',
     'Dispatches a Payment Attempt command to its configured provider'),
    ('refund.create_requested', 'payments.provider_dispatch',
     'Dispatches a Refund command to its configured provider'),
    ('search.product.changed', 'search.product_indexer',
     'Refreshes the Store-isolated Product search document'),
    ('fulfillment.shipped', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('fulfillment.delivered', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('fulfillment.cancelled', 'fulfillment.operations',
     'Reconciles Order fulfillment and delivery state'),
    ('return.completed', 'fulfillment.operations',
     'Coordinates the immutable Return refund'),
    ('analytics.order.created', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Order creation fact'),
    ('analytics.payment.captured', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Payment capture fact'),
    ('analytics.refund.succeeded', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Refund success fact'),
    ('analytics.fulfillment.shipped', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Fulfillment shipment fact'),
    ('analytics.return.completed', 'analytics.commerce_fact_ingestor',
     'Ingests an immutable Return completion fact');

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA identity TO chaos_control_plane;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA identity TO chaos_control_plane;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_control_plane;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_control_plane;

REVOKE ALL ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) FROM PUBLIC;

COMMENT ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) IS
    'Authenticates a machine credential without exposing stored secret digests';

GRANT EXECUTE
    ON FUNCTION merchant.authenticate_api_key(TEXT, BYTEA) TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA merchant TO chaos_runtime;

REVOKE UPDATE, DELETE ON merchant.store_locale_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA merchant TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA merchant
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

REVOKE ALL ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_fulfillment_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.claim_webhook_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_outbox_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.finish_webhook_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) FROM PUBLIC;

REVOKE ALL ON FUNCTION integration.event_consumer_backlog() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION integration.claim_outbox_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_fulfillment_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.claim_webhook_events(
    UUID, INTEGER, TIMESTAMPTZ, TIMESTAMPTZ
)
    TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_outbox_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.finish_webhook_event(
    UUID, UUID, BOOLEAN, TEXT, INTEGER, TIMESTAMPTZ
) TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.queue_metrics() TO chaos_runtime;

GRANT EXECUTE ON FUNCTION integration.event_consumer_backlog() TO chaos_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA integration TO chaos_runtime;

REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON integration.event_consumer_registry FROM chaos_runtime;

REVOKE UPDATE, DELETE
    ON integration.webhook_inbox, integration.outbox_events FROM chaos_runtime;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA integration TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;

ALTER DEFAULT PRIVILEGES IN SCHEMA integration
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

GRANT USAGE ON SCHEMA identity TO chaos_control_plane;

GRANT USAGE ON SCHEMA integration, merchant TO chaos_runtime;
