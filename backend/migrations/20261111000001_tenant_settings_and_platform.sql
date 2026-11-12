-- Tenant settings and quotas, platform super admins, tenant deletion
-- (L02-US01). Specification §3, §8, §14.
--
-- The services (mlbpp_app) never write the tenant table. Platform operations
-- go through the SECURITY DEFINER functions below, owned by mlbpp_owner,
-- which check by themselves that the acting account is a platform super
-- admin: an application bug cannot create or delete a tenant on behalf of
-- anyone else.

-- ---------------------------------------------------------------------------
-- Tenant settings
-- ---------------------------------------------------------------------------
ALTER TABLE tenant
    -- Default weekly capacity of new resources, in minutes per weekday,
    -- Monday first (§8): 7 h × 5 days by default.
    ADD COLUMN default_capacity_minutes integer[] NOT NULL DEFAULT '{420,420,420,420,420,0,0}'
        CONSTRAINT tenant_capacity_check CHECK (
            cardinality(default_capacity_minutes) = 7
            AND array_position(default_capacity_minutes, NULL) IS NULL
            AND 0 <= ALL (default_capacity_minutes)
            AND 1440 >= ALL (default_capacity_minutes)
        ),
    -- Load accepted above capacity before a day counts as overloaded (§8).
    ADD COLUMN overload_tolerance_percent smallint NOT NULL DEFAULT 0
        CONSTRAINT tenant_tolerance_check CHECK (overload_tolerance_percent BETWEEN 0 AND 100),
    -- blocking: an overloading assignment is refused; warning: accepted, flagged.
    ADD COLUMN overload_mode text NOT NULL DEFAULT 'blocking'
        CONSTRAINT tenant_overload_mode_check CHECK (overload_mode IN ('blocking', 'warning')),
    -- Quotas; NULL means unlimited.
    ADD COLUMN storage_quota_bytes bigint
        CONSTRAINT tenant_storage_quota_check CHECK (storage_quota_bytes > 0),
    ADD COLUMN api_rate_limit_per_minute integer
        CONSTRAINT tenant_api_rate_check CHECK (api_rate_limit_per_minute > 0),
    -- deleting: the tenant no longer resolves; a job purges its data.
    ADD COLUMN status text NOT NULL DEFAULT 'active'
        CONSTRAINT tenant_status_check CHECK (status IN ('active', 'deleting'));

-- The time zone must be one PostgreSQL knows (IANA names): the application
-- checks it first, this keeps the table right whatever writes it.
CREATE FUNCTION app_check_tenant_time_zone() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name = NEW.time_zone) THEN
        RAISE EXCEPTION 'unknown time zone %', NEW.time_zone USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER check_time_zone BEFORE INSERT OR UPDATE OF time_zone ON tenant
    FOR EACH ROW EXECUTE FUNCTION app_check_tenant_time_zone();

-- Resolution also returns the quotas the services enforce, and ignores
-- tenants being deleted: their host names stop resolving at once.
DROP FUNCTION app_resolve_tenant(text);
CREATE FUNCTION app_resolve_tenant(p_slug text)
    RETURNS TABLE (id uuid, default_locale text, storage_quota_bytes bigint, api_rate_limit_per_minute integer)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.default_locale, t.storage_quota_bytes, t.api_rate_limit_per_minute
    FROM public.tenant t
    WHERE t.slug = p_slug AND t.status = 'active'
$$;
REVOKE EXECUTE ON FUNCTION app_resolve_tenant(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_resolve_tenant(text) TO mlbpp_app;

-- ---------------------------------------------------------------------------
-- Platform super admins: a marker on global accounts, outside the tenants.
-- The services cannot read nor write it; app_is_platform_admin answers.
-- ---------------------------------------------------------------------------
CREATE TABLE platform_admin (
    account_id uuid PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE platform_admin ENABLE ROW LEVEL SECURITY;
ALTER TABLE platform_admin FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_admin_owner ON platform_admin TO mlbpp_owner USING (true) WITH CHECK (true);

-- Memberships of every tenant, for the platform functions (orphan accounts
-- after a tenant purge, bootstrap).
CREATE POLICY membership_platform ON membership TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE FUNCTION app_is_platform_admin(p_account uuid) RETURNS boolean
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS (SELECT 1 FROM public.platform_admin WHERE account_id = p_account)
$$;
REVOKE EXECUTE ON FUNCTION app_is_platform_admin(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_is_platform_admin(uuid) TO mlbpp_app;

-- Raises insufficient_privilege unless `p_actor` is a platform super admin.
CREATE FUNCTION app_require_platform_admin(p_actor uuid) RETURNS void
    LANGUAGE plpgsql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_actor IS NULL OR NOT EXISTS (SELECT 1 FROM public.platform_admin WHERE account_id = p_actor) THEN
        RAISE EXCEPTION 'platform super admin required' USING ERRCODE = 'insufficient_privilege';
    END IF;
END
$$;
REVOKE EXECUTE ON FUNCTION app_require_platform_admin(uuid) FROM PUBLIC;

-- ---------------------------------------------------------------------------
-- Tenant management, for platform super admins only.
-- ---------------------------------------------------------------------------

-- A page of tenants, in creation order, after (p_after_created, p_after_id).
CREATE FUNCTION app_platform_tenants(
    p_actor uuid, p_after_created timestamptz, p_after_id uuid, p_limit integer
) RETURNS SETOF public.tenant
    LANGUAGE plpgsql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    PERFORM app_require_platform_admin(p_actor);
    RETURN QUERY
        SELECT * FROM public.tenant t
        WHERE p_after_id IS NULL OR (t.created_at, t.id) > (p_after_created, p_after_id)
        ORDER BY t.created_at, t.id
        LIMIT p_limit;
END
$$;

CREATE FUNCTION app_platform_tenant(p_actor uuid, p_id uuid) RETURNS SETOF public.tenant
    LANGUAGE plpgsql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    PERFORM app_require_platform_admin(p_actor);
    RETURN QUERY SELECT * FROM public.tenant t WHERE t.id = p_id;
END
$$;

CREATE FUNCTION app_platform_create_tenant(
    p_actor uuid, p_id uuid, p_slug text, p_name text, p_default_locale text, p_time_zone text,
    p_capacity integer[], p_tolerance smallint, p_overload_mode text,
    p_storage_quota bigint, p_api_rate integer
) RETURNS SETOF public.tenant
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    PERFORM app_require_platform_admin(p_actor);
    RETURN QUERY
        INSERT INTO public.tenant (
            id, slug, name, default_locale, time_zone, default_capacity_minutes,
            overload_tolerance_percent, overload_mode, storage_quota_bytes, api_rate_limit_per_minute
        )
        VALUES (p_id, p_slug, p_name, p_default_locale, p_time_zone, p_capacity,
                p_tolerance, p_overload_mode, p_storage_quota, p_api_rate)
        RETURNING *;
END
$$;

-- Updates an active tenant whose version is one of `p_versions` (any when
-- NULL). No row: unknown, being deleted, or another version.
CREATE FUNCTION app_platform_update_tenant(
    p_actor uuid, p_id uuid, p_versions bigint[], p_name text, p_default_locale text, p_time_zone text,
    p_capacity integer[], p_tolerance smallint, p_overload_mode text,
    p_storage_quota bigint, p_api_rate integer
) RETURNS SETOF public.tenant
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    PERFORM app_require_platform_admin(p_actor);
    RETURN QUERY
        UPDATE public.tenant t
        SET name = p_name, default_locale = p_default_locale, time_zone = p_time_zone,
            default_capacity_minutes = p_capacity, overload_tolerance_percent = p_tolerance,
            overload_mode = p_overload_mode, storage_quota_bytes = p_storage_quota,
            api_rate_limit_per_minute = p_api_rate, version = t.version + 1
        WHERE t.id = p_id AND t.status = 'active' AND (p_versions IS NULL OR t.version = ANY (p_versions))
        RETURNING *;
END
$$;

-- Starts deleting an active tenant whose version is one of `p_versions`:
-- it stops resolving at once, and the purge job `p_job_id` removes its data.
CREATE FUNCTION app_platform_delete_tenant(p_actor uuid, p_id uuid, p_versions bigint[], p_job_id uuid)
    RETURNS SETOF public.tenant
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    deleting public.tenant;
BEGIN
    PERFORM app_require_platform_admin(p_actor);
    UPDATE public.tenant t
    SET status = 'deleting', version = t.version + 1
    WHERE t.id = p_id AND t.status = 'active' AND (p_versions IS NULL OR t.version = ANY (p_versions))
    RETURNING * INTO deleting;
    IF deleting.id IS NULL THEN
        RETURN;
    END IF;
    INSERT INTO public.job (id, tenant_id, kind, payload)
    VALUES (p_job_id, p_id, 'tenant.purge', '{}');
    RETURN NEXT deleting;
END
$$;

-- Removes every row of a tenant being deleted, then the tenant, then the
-- accounts that belonged to no other tenant. Run by the worker's purge job
-- (in that tenant's context); returns the number of rows removed.
--
-- Every table with a tenant_id column is purged, so new tables need nothing
-- here. Their order is unknown: a table still referenced by another one is
-- retried after the others.
CREATE FUNCTION app_platform_purge_tenant(p_id uuid) RETURNS bigint
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tables text[];
    blocked text[];
    members uuid[];
    target text;
    removed bigint := 0;
    n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.tenant WHERE id = p_id AND status = 'deleting') THEN
        RAISE EXCEPTION 'tenant % is not being deleted', p_id USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
    -- Row-level security of the business tables applies to the owner too.
    PERFORM set_config('app.tenant_id', p_id::text, true);

    SELECT coalesce(array_agg(account_id), '{}') INTO members FROM public.membership WHERE tenant_id = p_id;

    SELECT coalesce(array_agg(c.relname::text ORDER BY c.relname), '{}') INTO tables
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace ns ON ns.oid = c.relnamespace
    JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
    WHERE ns.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relispartition = false
      AND a.attname = 'tenant_id' AND NOT a.attisdropped;

    WHILE cardinality(tables) > 0 LOOP
        blocked := '{}';
        FOREACH target IN ARRAY tables LOOP
            BEGIN
                EXECUTE format('DELETE FROM public.%I WHERE tenant_id = $1', target) USING p_id;
                GET DIAGNOSTICS n = ROW_COUNT;
                removed := removed + n;
            EXCEPTION WHEN foreign_key_violation THEN
                blocked := blocked || target;
            END;
        END LOOP;
        IF cardinality(blocked) = cardinality(tables) THEN
            RAISE EXCEPTION 'cannot purge tenant %: % reference each other', p_id, blocked;
        END IF;
        tables := blocked;
    END LOOP;

    DELETE FROM public.tenant WHERE id = p_id;
    removed := removed + 1;

    -- Accounts that only existed for this tenant (GDPR: all its data goes).
    DELETE FROM public.account a
    WHERE a.id = ANY (members)
      AND NOT EXISTS (SELECT 1 FROM public.membership m WHERE m.account_id = a.id)
      AND NOT EXISTS (SELECT 1 FROM public.platform_admin p WHERE p.account_id = a.id);
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN removed + n;
END
$$;

REVOKE EXECUTE ON FUNCTION app_platform_tenants(uuid, timestamptz, uuid, integer) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_platform_tenant(uuid, uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_platform_create_tenant(uuid, uuid, text, text, text, text, integer[], smallint, text, bigint, integer) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_platform_update_tenant(uuid, uuid, bigint[], text, text, text, integer[], smallint, text, bigint, integer) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_platform_delete_tenant(uuid, uuid, bigint[], uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_platform_purge_tenant(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_platform_tenants(uuid, timestamptz, uuid, integer) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_platform_tenant(uuid, uuid) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_platform_create_tenant(uuid, uuid, text, text, text, text, integer[], smallint, text, bigint, integer) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_platform_update_tenant(uuid, uuid, bigint[], text, text, text, integer[], smallint, text, bigint, integer) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_platform_delete_tenant(uuid, uuid, bigint[], uuid) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_platform_purge_tenant(uuid) TO mlbpp_app;
