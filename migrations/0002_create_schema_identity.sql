CREATE SCHEMA identity;

COMMENT ON SCHEMA identity IS
    'Users, credentials, service accounts, and sessions';

CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');

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

CREATE INDEX magic_link_challenges_email_created_idx
    ON identity.magic_link_challenges (email, created_at DESC);

CREATE INDEX sessions_user_expires_idx
    ON identity.sessions (user_id, expires_at DESC);

CREATE INDEX passkey_credentials_user_created_idx
    ON identity.passkey_credentials (user_id, created_at DESC);

GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA identity TO chaos_control_plane;

GRANT USAGE, SELECT
    ON ALL SEQUENCES IN SCHEMA identity TO chaos_control_plane;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_control_plane;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_control_plane;

GRANT USAGE ON SCHEMA identity TO chaos_control_plane;
