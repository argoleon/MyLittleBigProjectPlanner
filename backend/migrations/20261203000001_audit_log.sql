-- The history of tickets (L05-US06). Specification §12, §18.
--
-- Every ticket write records here, in its own transaction, who changed
-- what, when and from where: one row per changed field (`updated`), one per
-- other action (creation, move, links, comments). Rows written by one
-- operation share `change_id`. Partitioned by month from the start (§18),
-- like the outbox: partitions created ahead by
-- `app_ensure_audit_partitions` (the migration and the worker call it), and
-- a default partition as a safety net.
CREATE TABLE audit_log (
    id          uuid NOT NULL,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    entity_type text NOT NULL CHECK (entity_type IN ('issue')),
    entity_id   uuid NOT NULL,
    change_id   uuid NOT NULL,
    action      text NOT NULL CHECK (action IN (
                    'created', 'updated', 'moved', 'link_added', 'link_removed',
                    'comment_created', 'comment_updated', 'comment_deleted')),
    -- The field of an `updated` row: `status`, `dueDate`…, `fields.{id}`.
    field       text CHECK ((action = 'updated') = (field IS NOT NULL)),
    old_value   jsonb,
    new_value   jsonb,
    -- Who made the change; none for the system. An erased account shows
    -- under its pseudonym (`app_member_name`).
    actor_id    uuid,
    origin      text NOT NULL CHECK (origin IN ('interface', 'api', 'mcp', 'import', 'system')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);
SELECT app_setup_business_table('audit_log');
CREATE INDEX audit_log_entity_idx ON audit_log (tenant_id, entity_id, created_at);

CREATE TABLE audit_log_default PARTITION OF audit_log DEFAULT;
SELECT app_secure_partition('audit_log_default');

-- The partitions of the current month and of the `months_ahead` next ones,
-- created if missing; the number created. The worker calls it every day.
CREATE FUNCTION app_ensure_audit_partitions(months_ahead integer) RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    month   date := date_trunc('month', now() AT TIME ZONE 'UTC')::date;
    name    text;
    created integer := 0;
BEGIN
    FOR i IN 0..greatest(months_ahead, 0) LOOP
        name := format('audit_log_%s', to_char(month + make_interval(months => i), 'YYYY_MM'));
        IF to_regclass('public.' || name) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE public.%I PARTITION OF public.audit_log FOR VALUES FROM (%L) TO (%L)',
                name,
                (month + make_interval(months => i))::timestamp AT TIME ZONE 'UTC',
                (month + make_interval(months => i + 1))::timestamp AT TIME ZONE 'UTC');
            PERFORM public.app_secure_partition(('public.' || name)::regclass);
            created := created + 1;
        END IF;
    END LOOP;
    RETURN created;
END
$$;
REVOKE EXECUTE ON FUNCTION app_ensure_audit_partitions(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_ensure_audit_partitions(integer) TO mlbpp_app;
SELECT app_ensure_audit_partitions(3);

-- The name a member shows under in the current tenant: "Deleted user #N"
-- once erased there (the account keeps its name when erased in this tenant
-- only), else the account's. Row-level security applies: the caller's
-- tenant only.
CREATE FUNCTION app_member_name(p_account uuid) RETURNS text
    LANGUAGE sql STABLE
    SET search_path = pg_catalog, public
AS $$
    SELECT coalesce(
        (SELECT 'Deleted user #' || pseudonym_number FROM public.erased_member WHERE account_id = p_account),
        (SELECT display_name FROM public.account WHERE id = p_account))
$$;
GRANT EXECUTE ON FUNCTION app_member_name(uuid) TO mlbpp_app;
