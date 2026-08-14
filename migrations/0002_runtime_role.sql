DO $$
BEGIN
    CREATE ROLE chaos_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

GRANT chaos_runtime TO chaos;
GRANT USAGE ON SCHEMA public TO chaos_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO chaos_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO chaos_runtime;

ALTER DEFAULT PRIVILEGES FOR ROLE chaos IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO chaos_runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE chaos IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO chaos_runtime;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';
