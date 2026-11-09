-- Tenancy foundation: conventions shared by every business table, and the
-- tenant table. See backend/migrations/README.md.
--
-- Roles (created by the deployment, not by migrations):
--   mlbpp_owner  owns the schema and runs migrations
--   mlbpp_app    used by the services at runtime; not owner, no BYPASSRLS

-- Tenant of the current transaction, set by the request layer with
-- set_config('app.tenant_id', <uuid>, true). NULL when unset, so that no
-- row ever matches outside a tenant context.
CREATE FUNCTION app_current_tenant() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid
$$;

CREATE FUNCTION app_touch_updated_at() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END
$$;

-- Applies the business table conventions to `target`, which must have a
-- `tenant_id uuid NOT NULL` and an `updated_at timestamptz` column:
--   * row-level security, enabled and forced, restricted to the current tenant
--     for reads and writes;
--   * read and write grants for the application role (nothing more);
--   * `updated_at` maintained on every update.
-- Every migration creating a business table calls it right after CREATE TABLE.
CREATE FUNCTION app_setup_business_table(target regclass) RETURNS void
    LANGUAGE plpgsql
AS $$
BEGIN
    EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', target);
    EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', target);
    EXECUTE format(
        'CREATE POLICY tenant_isolation ON %s '
        'USING (tenant_id = app_current_tenant()) '
        'WITH CHECK (tenant_id = app_current_tenant())',
        target);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %s TO mlbpp_app', target);
    EXECUTE format(
        'CREATE TRIGGER touch_updated_at BEFORE UPDATE ON %s '
        'FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at()',
        target);
END
$$;
REVOKE EXECUTE ON FUNCTION app_setup_business_table(regclass) FROM PUBLIC;

-- Tenants. Not a tenant-scoped table: its key *is* the tenant. Row-level
-- security is enabled but not forced, so that the owner-defined resolver
-- below can look tenants up by slug before any tenant context exists.
CREATE TABLE tenant (
    id             uuid PRIMARY KEY,
    slug           text NOT NULL UNIQUE
                   CHECK (slug ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
    name           text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    default_locale text NOT NULL DEFAULT 'fr' CHECK (default_locale IN ('fr', 'en')),
    time_zone      text NOT NULL DEFAULT 'Europe/Paris',
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1
);

ALTER TABLE tenant ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_self ON tenant FOR SELECT TO mlbpp_app
    USING (id = app_current_tenant());
GRANT SELECT ON tenant TO mlbpp_app;
CREATE TRIGGER touch_updated_at BEFORE UPDATE ON tenant
    FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at();

-- Resolves a slug to a tenant id without exposing the tenant list.
CREATE FUNCTION app_resolve_tenant(p_slug text) RETURNS uuid
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT id FROM public.tenant WHERE slug = p_slug
$$;
REVOKE EXECUTE ON FUNCTION app_resolve_tenant(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_resolve_tenant(text) TO mlbpp_app;
