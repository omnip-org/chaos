REVOKE CREATE ON SCHEMA public FROM PUBLIC;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';

COMMENT ON ROLE chaos_control_plane IS
    'Non-owner identity role. It cannot access merchant-owned tables.';
