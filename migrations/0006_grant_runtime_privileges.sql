REVOKE CREATE ON SCHEMA public FROM PUBLIC;

GRANT USAGE ON SCHEMA extensions, integration, merchant, catalog, pricing, inventory, sales,
    payments, fulfillment, notification, analytics, search
    TO chaos_runtime;

GRANT USAGE ON SCHEMA extensions, identity TO chaos_control_plane;

COMMENT ON ROLE chaos_runtime IS
    'Non-owner application role. RLS applies; login roles must SET ROLE chaos_runtime.';

COMMENT ON ROLE chaos_control_plane IS
    'Non-owner identity role. It cannot access merchant-owned tables.';
