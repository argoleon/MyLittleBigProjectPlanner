-- The overload check (specification §8, L08-US03).
--
-- * A tenant setting tells whether provisional assignments count in the
--   check (they do by default); the tenant admin changes it with the other
--   workload settings.
-- * A forced overload is traced twice: in the ticket's history
--   (`overload_forced`), and in `forced_overload`, which tenant admins read.

ALTER TABLE tenant ADD COLUMN overload_counts_provisional boolean NOT NULL DEFAULT true;

-- The function now takes the new setting too.
DROP FUNCTION app_update_workload_settings(uuid, bigint[], integer[], smallint, text);

-- Changes the workload settings of the current tenant for `p_actor`, who
-- must be one of its active internal tenant admins, if the tenant's version
-- is one of `p_versions` (NULL: any). Returns the tenant row, none when
-- the version does not match.
CREATE FUNCTION app_update_workload_settings(
    p_actor uuid, p_versions bigint[], p_capacity integer[], p_tolerance smallint, p_overload_mode text,
    p_counts_provisional boolean
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
            overload_mode = p_overload_mode, overload_counts_provisional = p_counts_provisional,
            version = t.version + 1
        WHERE t.id = app_current_tenant() AND t.status = 'active'
          AND (p_versions IS NULL OR t.version = ANY (p_versions))
        RETURNING *;
END
$$;
REVOKE EXECUTE ON FUNCTION app_update_workload_settings(uuid, bigint[], integer[], smallint, text, boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_update_workload_settings(uuid, bigint[], integer[], smallint, text, boolean) TO mlbpp_app;

-- The history records forced overloads.
ALTER TABLE audit_log DROP CONSTRAINT audit_log_action_check;
ALTER TABLE audit_log ADD CONSTRAINT audit_log_action_check CHECK (action IN (
    'created', 'updated', 'moved', 'link_added', 'link_removed',
    'comment_created', 'comment_updated', 'comment_deleted',
    'attachment_added', 'attachment_removed', 'overload_forced'));

-- Every assignment written beyond capacity plus tolerance in blocking mode
-- by a member allowed to force it. Kept when the assignment, its ticket or
-- its resource goes: no foreign keys, the names are read while they exist.
CREATE TABLE forced_overload (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    assignment_id uuid NOT NULL,
    issue_id      uuid NOT NULL,
    resource_id   uuid NOT NULL,
    -- Who forced it. An erased account shows under its pseudonym
    -- (`app_member_name`).
    actor_id      uuid NOT NULL,
    origin        text NOT NULL CHECK (origin IN ('interface', 'api', 'mcp', 'import', 'system')),
    -- The days overloaded: [{"date", "plannedMinutes", "availableMinutes", "limitMinutes"}].
    days          jsonb NOT NULL CHECK (jsonb_typeof(days) = 'array' AND jsonb_array_length(days) > 0),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
SELECT app_setup_business_table('forced_overload');
-- Newest first.
CREATE INDEX forced_overload_tenant_created_idx ON forced_overload (tenant_id, created_at DESC, id DESC);
