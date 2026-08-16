ALTER TABLE analytics.destination_accounts
    DROP CONSTRAINT destination_accounts_secret_reference_check;

ALTER TABLE analytics.destination_accounts
    ADD CONSTRAINT destination_accounts_secret_reference_check CHECK (
        credential_secret_reference ~ '^env://CHAOS_ANALYTICS_SECRET_[A-Z0-9_]{1,96}$'
        OR (
            char_length(credential_secret_reference) <= 255
            AND credential_secret_reference ~ '^vault://[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)+$'
            AND credential_secret_reference !~ '(^|/)\.{1,2}(/|$)'
        )
    );
