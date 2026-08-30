CREATE SCHEMA identity;

CREATE TYPE identity.user_status AS ENUM ('active', 'disabled');
CREATE TYPE identity.identity_provider AS ENUM ('apple', 'google');

CREATE TABLE identity.users (
    id          UUID                     NOT NULL,
    email       extensions.citext        NOT NULL,
    status      identity.user_status     NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT users_pkey                PRIMARY KEY (id),
    CONSTRAINT users_email_key           UNIQUE (email),
    CONSTRAINT users_email_length_check  CHECK (length(trim(email::text)) BETWEEN 3 AND 320)
);

CREATE TABLE identity.credentials (
    provider    identity.identity_provider  NOT NULL,
    subject     TEXT                        NOT NULL,
    user_id     UUID                        NOT NULL,
    email       extensions.citext           NOT NULL,
    meta        JSONB,
    created_at  TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ                 NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT credentials_provider_user_id_key      UNIQUE (provider, user_id),
    CONSTRAINT credentials_pkey                      PRIMARY KEY (provider, subject),
    CONSTRAINT credentials_user_id_fkey              FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE,
    CONSTRAINT credentials_meta_size_check           CHECK (meta IS NULL OR pg_column_size(meta) <= 32768),
    CONSTRAINT credentials_meta_is_object_check      CHECK (meta IS NULL OR jsonb_typeof(meta) = 'object'),
    CONSTRAINT credentials_subject_length_check      CHECK (octet_length(subject) BETWEEN 1 AND 255),
    CONSTRAINT credentials_email_length_check        CHECK (length(trim(email::text)) BETWEEN 3 AND 320)
);

CREATE INDEX credentials_user_id_idx ON identity.credentials (user_id, provider);

CREATE TABLE identity.oauth_clients (
    client_id                       TEXT                  NOT NULL,
    client_name                     TEXT                  NOT NULL,
    redirect_uris                   JSONB                 NOT NULL,
    grant_types                     JSONB                 NOT NULL,
    response_types                  JSONB                 NOT NULL,
    token_endpoint_auth_method      TEXT                  NOT NULL,
    application_type                TEXT                  NOT NULL,
    created_at                      TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ           NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_clients_pkey                         PRIMARY KEY (client_id),
    CONSTRAINT oauth_clients_client_id_length_check       CHECK (length(client_id) BETWEEN 1 AND 160),
    CONSTRAINT oauth_clients_name_length_check            CHECK (length(trim(client_name)) BETWEEN 1 AND 120),
    CONSTRAINT oauth_clients_redirect_uris_array_check    CHECK (jsonb_typeof(redirect_uris) = 'array'),
    CONSTRAINT oauth_clients_grant_types_array_check      CHECK (jsonb_typeof(grant_types) = 'array'),
    CONSTRAINT oauth_clients_response_types_array_check   CHECK (jsonb_typeof(response_types) = 'array'),
    CONSTRAINT oauth_clients_redirect_uris_size_check     CHECK (jsonb_array_length(CASE WHEN jsonb_typeof(redirect_uris) = 'array' THEN redirect_uris ELSE '[]'::jsonb END) BETWEEN 1 AND 100 AND octet_length(redirect_uris::text) <= 32768),
    CONSTRAINT oauth_clients_grant_types_size_check       CHECK (jsonb_array_length(CASE WHEN jsonb_typeof(grant_types) = 'array' THEN grant_types ELSE '[]'::jsonb END) BETWEEN 1 AND 32 AND octet_length(grant_types::text) <= 8192),
    CONSTRAINT oauth_clients_response_types_size_check    CHECK (jsonb_array_length(CASE WHEN jsonb_typeof(response_types) = 'array' THEN response_types ELSE '[]'::jsonb END) BETWEEN 1 AND 32 AND octet_length(response_types::text) <= 8192),
    CONSTRAINT oauth_clients_auth_method_check            CHECK (token_endpoint_auth_method = 'none'),
    CONSTRAINT oauth_clients_application_type_check       CHECK (application_type IN ('native', 'web'))
);

CREATE TABLE identity.oauth_authorization_requests (
    id                          UUID                            NOT NULL,
    client_id                   TEXT                            NOT NULL,
    redirect_uri                TEXT                            NOT NULL,
    scope                       TEXT                            NOT NULL,
    state                       TEXT,
    code_challenge              TEXT                            NOT NULL,
    code_challenge_method       TEXT                            NOT NULL,
    resource                    TEXT                            NOT NULL,
    expires_at                  TIMESTAMPTZ                     NOT NULL,
    used_at                    TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ                     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_authorization_requests_pkey                PRIMARY KEY (id),
    CONSTRAINT oauth_authorization_requests_client_fkey         FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_authorization_requests_redirect_uri_check  CHECK (length(trim(redirect_uri)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_requests_scope_check         CHECK (length(trim(scope)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_requests_state_check         CHECK (state IS NULL OR length(state) BETWEEN 1 AND 1024),
    CONSTRAINT oauth_authorization_requests_challenge_check     CHECK (length(code_challenge) BETWEEN 43 AND 128),
    CONSTRAINT oauth_authorization_requests_resource_check      CHECK (length(trim(resource)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_requests_pkce_method_check   CHECK (code_challenge_method = 'S256')
);

CREATE TABLE identity.oauth_authorization_codes (
    code_digest                 BYTEA                         NOT NULL,
    client_id                   TEXT                          NOT NULL,
    user_id                     UUID                          NOT NULL,
    redirect_uri                TEXT                          NOT NULL,
    scope                       TEXT                          NOT NULL,
    code_challenge              TEXT                          NOT NULL,
    code_challenge_method       TEXT                          NOT NULL,
    resource                    TEXT                          NOT NULL,
    expires_at                  TIMESTAMPTZ                   NOT NULL,
    consumed_at                TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ                   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_authorization_codes_pkey                 PRIMARY KEY (code_digest),
    CONSTRAINT oauth_authorization_codes_digest_length_check  CHECK (octet_length(code_digest) = 32),
    CONSTRAINT oauth_authorization_codes_client_fkey          FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_authorization_codes_user_fkey            FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE,
    CONSTRAINT oauth_authorization_codes_redirect_uri_check   CHECK (length(trim(redirect_uri)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_codes_scope_check          CHECK (length(trim(scope)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_codes_challenge_check      CHECK (length(code_challenge) BETWEEN 43 AND 128),
    CONSTRAINT oauth_authorization_codes_resource_check       CHECK (length(trim(resource)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_authorization_codes_pkce_method_check    CHECK (code_challenge_method = 'S256')
);

CREATE TABLE identity.oauth_access_tokens (
    token_digest                BYTEA                   NOT NULL,
    client_id                   TEXT                    NOT NULL,
    user_id                     UUID                    NOT NULL,
    scope                       TEXT                    NOT NULL,
    resource                    TEXT                    NOT NULL,
    expires_at                  TIMESTAMPTZ             NOT NULL,
    revoked_at                  TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ             NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_access_tokens_pkey                 PRIMARY KEY (token_digest),
    CONSTRAINT oauth_access_tokens_digest_length_check  CHECK (octet_length(token_digest) = 32),
    CONSTRAINT oauth_access_tokens_client_fkey          FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_access_tokens_user_fkey            FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE,
    CONSTRAINT oauth_access_tokens_scope_check          CHECK (length(trim(scope)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_access_tokens_resource_check       CHECK (length(trim(resource)) BETWEEN 1 AND 2048)
);

CREATE TABLE identity.oauth_refresh_tokens (
    token_digest                BYTEA                    NOT NULL,
    client_id                   TEXT                     NOT NULL,
    user_id                     UUID                     NOT NULL,
    scope                       TEXT                     NOT NULL,
    resource                    TEXT                     NOT NULL,
    expires_at                  TIMESTAMPTZ              NOT NULL,
    revoked_at                  TIMESTAMPTZ,
    replaced_by_digest          BYTEA,
    created_at                  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_refresh_tokens_pkey                          PRIMARY KEY (token_digest),
    CONSTRAINT oauth_refresh_tokens_digest_length_check           CHECK (octet_length(token_digest) = 32),
    CONSTRAINT oauth_refresh_tokens_client_fkey                   FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_refresh_tokens_user_fkey                     FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE,
    CONSTRAINT oauth_refresh_tokens_scope_check                   CHECK (length(trim(scope)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_refresh_tokens_resource_check                CHECK (length(trim(resource)) BETWEEN 1 AND 2048),
    CONSTRAINT oauth_refresh_tokens_replaced_digest_length_check  CHECK (replaced_by_digest IS NULL OR octet_length(replaced_by_digest) = 32)
);

CREATE INDEX oauth_authorization_requests_expiry_idx ON identity.oauth_authorization_requests (expires_at, created_at, id);
CREATE INDEX oauth_authorization_requests_used_cleanup_idx ON identity.oauth_authorization_requests (used_at, created_at, id) WHERE used_at IS NOT NULL;
CREATE INDEX oauth_authorization_requests_client_idx ON identity.oauth_authorization_requests (client_id, id);
CREATE INDEX oauth_authorization_codes_expiry_idx ON identity.oauth_authorization_codes (expires_at, created_at, code_digest);
CREATE INDEX oauth_authorization_codes_consumed_cleanup_idx ON identity.oauth_authorization_codes (consumed_at, created_at, code_digest) WHERE consumed_at IS NOT NULL;
CREATE INDEX oauth_authorization_codes_client_idx ON identity.oauth_authorization_codes (client_id, code_digest);
CREATE INDEX oauth_authorization_codes_user_idx ON identity.oauth_authorization_codes (user_id, code_digest);
CREATE INDEX oauth_access_tokens_user_id_idx ON identity.oauth_access_tokens (user_id, expires_at);
CREATE INDEX oauth_access_tokens_expiry_idx ON identity.oauth_access_tokens (expires_at, created_at, token_digest);
CREATE INDEX oauth_access_tokens_revoked_cleanup_idx ON identity.oauth_access_tokens (revoked_at, created_at, token_digest) WHERE revoked_at IS NOT NULL;
CREATE INDEX oauth_access_tokens_client_idx ON identity.oauth_access_tokens (client_id, token_digest);
CREATE INDEX oauth_refresh_tokens_user_id_idx ON identity.oauth_refresh_tokens (user_id, expires_at);
CREATE INDEX oauth_refresh_tokens_expiry_idx ON identity.oauth_refresh_tokens (expires_at, created_at, token_digest);
CREATE INDEX oauth_refresh_tokens_revoked_cleanup_idx ON identity.oauth_refresh_tokens (revoked_at, created_at, token_digest) WHERE revoked_at IS NOT NULL;
CREATE INDEX oauth_refresh_tokens_client_idx ON identity.oauth_refresh_tokens (client_id, token_digest);
CREATE INDEX oauth_refresh_tokens_active_family_idx ON identity.oauth_refresh_tokens (client_id, user_id, created_at DESC, token_digest) WHERE revoked_at IS NULL;

GRANT USAGE ON SCHEMA identity TO chaos_identity;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA identity TO chaos_identity;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA identity TO chaos_identity;

ALTER DEFAULT PRIVILEGES IN SCHEMA identity GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_identity;
ALTER DEFAULT PRIVILEGES IN SCHEMA identity GRANT USAGE, SELECT ON SEQUENCES TO chaos_identity;
