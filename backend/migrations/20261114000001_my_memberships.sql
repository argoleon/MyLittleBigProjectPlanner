-- Memberships of the signed-in account across tenants (L02-US05), for the
-- tenant switcher. Specification §3.
--
-- Row-level security shows one tenant at a time; this owner-defined
-- function is the only path across tenants, and it answers for the caller
-- only: the account set by the authenticated request layer
-- (set_config('app.account_id', …, true), next to app.tenant_id), never an
-- account passed as argument.

CREATE FUNCTION app_current_account() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('app.account_id', true), '')::uuid
$$;

CREATE FUNCTION app_my_memberships()
    RETURNS TABLE (
        tenant_id uuid, slug text, name text, account_type text, tenant_role text
    )
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.slug, t.name, m.account_type, m.tenant_role
    FROM public.membership m
    JOIN public.tenant t ON t.id = m.tenant_id
    WHERE m.account_id = app_current_account()
      AND m.status = 'active'
      AND t.status = 'active'
    ORDER BY t.name, t.id
$$;
REVOKE EXECUTE ON FUNCTION app_my_memberships() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_my_memberships() TO mlbpp_app;
