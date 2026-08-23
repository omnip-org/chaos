CREATE SCHEMA extensions;
CREATE SCHEMA partman;

CREATE EXTENSION citext WITH SCHEMA extensions;
CREATE EXTENSION IF NOT EXISTS pg_partman WITH SCHEMA partman;
CREATE EXTENSION IF NOT EXISTS pg_cron;
CREATE EXTENSION IF NOT EXISTS pgmq;

DO $$
BEGIN
    CREATE ROLE chaos_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    CREATE ROLE chaos_identity NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    EXECUTE format('GRANT chaos_runtime TO %I', current_user);
    EXECUTE format('GRANT chaos_identity TO %I', current_user);
END
$$;

GRANT USAGE ON SCHEMA extensions TO chaos_runtime;
GRANT USAGE ON SCHEMA extensions TO chaos_identity;
