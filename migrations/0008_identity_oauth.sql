-- OAuth 2.1 / PKCE state.  Secrets are never stored in plaintext; only
-- SHA-256 digests of authorization codes and bearer tokens are persisted.

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
    CONSTRAINT oauth_clients_auth_method_check            CHECK (token_endpoint_auth_method = 'none'),
    CONSTRAINT oauth_clients_application_type_check       CHECK (application_type IN ('native', 'web'))
);

CREATE TABLE identity.oauth_authorization_requests (
    id                          UUID                     NOT NULL,
    client_id                   TEXT                     NOT NULL,
    redirect_uri                TEXT                     NOT NULL,
    scope                       TEXT                     NOT NULL,
    state                       TEXT,
    code_challenge              TEXT                     NOT NULL,
    code_challenge_method       TEXT                     NOT NULL,
    resource                    TEXT                     NOT NULL,
    expires_at                  TIMESTAMPTZ              NOT NULL,
    used_at                     TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_authorization_requests_pkey              PRIMARY KEY (id),
    CONSTRAINT oauth_authorization_requests_client_fkey       FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_authorization_requests_pkce_method_check CHECK (code_challenge_method = 'S256')
);

CREATE TABLE identity.oauth_authorization_codes (
    code_digest                 BYTEA                    NOT NULL,
    client_id                   TEXT                     NOT NULL,
    user_id                     UUID                     NOT NULL,
    redirect_uri                TEXT                     NOT NULL,
    scope                       TEXT                     NOT NULL,
    code_challenge              TEXT                     NOT NULL,
    code_challenge_method       TEXT                     NOT NULL,
    resource                    TEXT                     NOT NULL,
    expires_at                  TIMESTAMPTZ              NOT NULL,
    consumed_at                 TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ              NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT oauth_authorization_codes_pkey                 PRIMARY KEY (code_digest),
    CONSTRAINT oauth_authorization_codes_digest_length_check  CHECK (octet_length(code_digest) = 32),
    CONSTRAINT oauth_authorization_codes_client_fkey          FOREIGN KEY (client_id) REFERENCES identity.oauth_clients (client_id) ON DELETE CASCADE,
    CONSTRAINT oauth_authorization_codes_user_fkey            FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE,
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
    CONSTRAINT oauth_access_tokens_user_fkey            FOREIGN KEY (user_id) REFERENCES identity.users (id) ON DELETE CASCADE
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
    CONSTRAINT oauth_refresh_tokens_replaced_digest_length_check  CHECK (replaced_by_digest IS NULL OR octet_length(replaced_by_digest) = 32)
);

CREATE INDEX oauth_authorization_requests_expiry_idx ON identity.oauth_authorization_requests (expires_at);
CREATE INDEX oauth_authorization_codes_expiry_idx ON identity.oauth_authorization_codes (expires_at);
CREATE INDEX oauth_access_tokens_user_id_idx ON identity.oauth_access_tokens (user_id, expires_at);
CREATE INDEX oauth_refresh_tokens_user_id_idx ON identity.oauth_refresh_tokens (user_id, expires_at);

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA identity TO chaos_identity;
ALTER DEFAULT PRIVILEGES IN SCHEMA identity GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_identity;
