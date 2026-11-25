-- Capacity patterns (specification §8, L07-US03): a resource's minutes per
-- weekday, historized by effective date.
--
-- * A period applies from `effective_from` until the next period of the
--   resource; before its first period, or without any, the tenant's
--   default capacity applies as it stands (never copied).
-- * One period per resource and effective date.
-- * The tenant admin changes the tenant's workload settings (default
--   capacity, overload tolerance and mode) through
--   `app_update_workload_settings`: the services never write the tenant
--   table themselves.

CREATE TABLE capacity_pattern (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id    uuid NOT NULL,
    -- A local date (the resource's calendar), never an instant.
    effective_from date NOT NULL,
    -- Minutes per weekday, Monday first, a full day at most.
    minutes        integer[] NOT NULL CHECK (
        cardinality(minutes) = 7
        AND array_position(minutes, NULL) IS NULL
        AND 0 <= ALL (minutes)
        AND 1440 >= ALL (minutes)
    ),
    -- "Part time 80%"…
    label          text CHECK (label IS NULL OR (length(label) BETWEEN 1 AND 100 AND label = btrim(label))),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1,
    CONSTRAINT capacity_pattern_resource_date_key UNIQUE (tenant_id, resource_id, effective_from),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('capacity_pattern');

-- Changes the workload settings of the current tenant for `p_actor`, who
-- must be one of its active internal tenant admins, if the tenant's version
-- is one of `p_versions` (NULL: any). Returns the tenant row, none when
-- the version does not match.
CREATE FUNCTION app_update_workload_settings(
    p_actor uuid, p_versions bigint[], p_capacity integer[], p_tolerance smallint, p_overload_mode text
) RETURNS SETOF public.tenant
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.membership m
        WHERE m.tenant_id = app_current_tenant() AND m.account_id = p_actor
          AND m.tenant_role = 'tenant_admin' AND m.status = 'active' AND m.account_type = 'internal'
    ) THEN
        RAISE EXCEPTION 'tenant admins only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN QUERY
        UPDATE public.tenant t
        SET default_capacity_minutes = p_capacity, overload_tolerance_percent = p_tolerance,
            overload_mode = p_overload_mode, version = t.version + 1
        WHERE t.id = app_current_tenant() AND t.status = 'active'
          AND (p_versions IS NULL OR t.version = ANY (p_versions))
        RETURNING *;
END
$$;
REVOKE EXECUTE ON FUNCTION app_update_workload_settings(uuid, bigint[], integer[], smallint, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_update_workload_settings(uuid, bigint[], integer[], smallint, text) TO mlbpp_app;
