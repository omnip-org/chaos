CREATE SCHEMA identity;

COMMENT ON SCHEMA identity IS
    'Users and external login identities';

-- Types

CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');

CREATE TYPE identity.identity_provider AS ENUM ('apple', 'google');

-- Tables

CREATE TABLE identity.users (
    id          UUID                    NOT NULL,
    email       extensions.citext       NOT NULL,
    status      identity.user_status    NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_email_key UNIQUE (email),
    CONSTRAINT users_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    )
);

CREATE TABLE identity.external_identities (
    provider    identity.identity_provider  NOT NULL,
    subject     TEXT                        NOT NULL,
    user_id     UUID                        NOT NULL,
    email       extensions.citext           NOT NULL,
    created_at  TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT external_identities_pkey PRIMARY KEY (provider, subject),
    CONSTRAINT external_identities_user_id_fkey FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT external_identities_provider_user_id_key UNIQUE (provider, user_id),
    CONSTRAINT external_identities_subject_length_check CHECK (
        octet_length(subject) BETWEEN 1 AND 255
    ),
    CONSTRAINT external_identities_email_length_check CHECK (
        length(trim(email::text)) BETWEEN 3 AND 320
    )
);

CREATE TABLE identity.access_keys (
    id              UUID                NOT NULL,
    user_id         UUID                NOT NULL,
    key_identifier  TEXT                NOT NULL,
    secret_digest   BYTEA               NOT NULL,
    display_suffix  CHAR(4)             NOT NULL,
    name            TEXT                NOT NULL,
    expires_at      TIMESTAMPTZ,
    last_used_at    TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at      TIMESTAMPTZ         NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT access_keys_pkey PRIMARY KEY (id),
    CONSTRAINT access_keys_user_id_fkey FOREIGN KEY (user_id)
        REFERENCES identity.users(id) ON DELETE CASCADE,
    CONSTRAINT access_keys_key_identifier_key UNIQUE (key_identifier),
    CONSTRAINT access_keys_key_identifier_format_check CHECK (
        key_identifier ~ '^[A-Za-z0-9_-]{16}$'
    ),
    CONSTRAINT access_keys_secret_digest_length_check CHECK (
        octet_length(secret_digest) = 32
    ),
    CONSTRAINT access_keys_display_suffix_format_check CHECK (
        display_suffix ~ '^[A-Za-z0-9_-]{4}$'
    ),
    CONSTRAINT access_keys_name_length_check CHECK (
        length(trim(name)) BETWEEN 1 AND 80
    ),
    CONSTRAINT access_keys_expiration_check CHECK (
        expires_at IS NULL OR expires_at > created_at
    )
);

-- Indexes

CREATE INDEX external_identities_user_idx
    ON identity.external_identities (user_id, provider);

CREATE INDEX access_keys_user_id_idx
    ON identity.access_keys (user_id, id);

-- Privileges

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA identity TO chaos_identity;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA identity TO chaos_identity;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_identity;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_identity;

GRANT USAGE ON SCHEMA identity TO chaos_identity;
