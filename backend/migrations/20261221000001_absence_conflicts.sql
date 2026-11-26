-- Absence conflicts (specification §8, §12, L08-US04).
--
-- * `workload_conflict`: per assignment and day, the hours planned in
--   hours per day or total effort that an absence of the resource leaves
--   no room for (domain::conflicts). Kept up to date with the daily load
--   (infra::conflicts, from infra::day_load::recompute): opened when they
--   appear, resolved when they no longer do, reopened if they come back.
-- * A new notification event, `absence_conflict`, in the rules of every
--   configuration: by default the ticket's assignee and the holders of the
--   Project manager role.

CREATE TABLE workload_conflict (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    assignment_id uuid NOT NULL,
    resource_id   uuid NOT NULL,
    issue_id      uuid NOT NULL,
    -- A local date of the resource.
    day           date NOT NULL,
    -- To reschedule; the last value once resolved.
    minutes       integer NOT NULL CHECK (minutes > 0),
    status        text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    opened_at     timestamptz NOT NULL DEFAULT now(),
    resolved_at   timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT workload_conflict_resolved CHECK ((status = 'resolved') = (resolved_at IS NOT NULL)),
    CONSTRAINT workload_conflict_assignment_day_key UNIQUE (tenant_id, assignment_id, day),
    -- Its conflicts go with the assignment (and the assignment with its ticket).
    FOREIGN KEY (tenant_id, assignment_id) REFERENCES assignment (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('workload_conflict');
-- The resource page (its absences) and the list by resource.
CREATE INDEX workload_conflict_resource_idx ON workload_conflict (tenant_id, resource_id, day);
-- The ticket's assignments and the list by project.
CREATE INDEX workload_conflict_issue_idx ON workload_conflict (tenant_id, issue_id);

-- The new event in the rules and the notifications.
ALTER TABLE notification_rule DROP CONSTRAINT notification_rule_event_check;
ALTER TABLE notification_rule ADD CONSTRAINT notification_rule_event_check
    CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned', 'absence_conflict'));
ALTER TABLE notification DROP CONSTRAINT notification_event_check;
ALTER TABLE notification ADD CONSTRAINT notification_event_check
    CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned', 'absence_conflict',
                     'export_ready', 'bulk_done'));

-- The rule of `absence_conflict` of a version: the assignee and the
-- tenant's Project manager role, if it still has one by that name.
CREATE FUNCTION app_seed_absence_conflict_rule(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant  uuid;
    manager uuid;
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    SELECT r.id INTO manager FROM public.role r
    WHERE r.tenant_id = tenant AND r.label ->> 'en' = 'Project manager'
    ORDER BY r.created_at, r.id
    LIMIT 1;
    INSERT INTO public.notification_rule (id, tenant_id, version_id, event, recipient_kinds, role_ids)
    VALUES (uuidv7(), tenant, p_version, 'absence_conflict', ARRAY['assignee'],
            CASE WHEN manager IS NULL THEN '{}'::uuid[] ELSE ARRAY[manager] END)
    ON CONFLICT (version_id, event) DO NOTHING;
END
$$;

-- The default rules of a version (domain::notifications::DEFAULT_RULES; an
-- integration test keeps both in step), as Jira's default scheme.
CREATE OR REPLACE FUNCTION app_seed_default_notifications(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant uuid;
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    INSERT INTO public.notification_rule (id, tenant_id, version_id, event, recipient_kinds)
    VALUES
        (uuidv7(), tenant, p_version, 'created', ARRAY['reporter', 'assignee', 'watchers']),
        (uuidv7(), tenant, p_version, 'assigned', ARRAY['assignee']),
        (uuidv7(), tenant, p_version, 'status_changed', ARRAY['reporter', 'assignee', 'watchers']),
        (uuidv7(), tenant, p_version, 'commented', ARRAY['reporter', 'assignee', 'watchers']),
        (uuidv7(), tenant, p_version, 'mentioned', ARRAY['mentioned']);
    PERFORM public.app_seed_absence_conflict_rule(p_version);
END
$$;

-- The versions made before.
SELECT app_seed_absence_conflict_rule(v.id)
FROM project_config_version v
WHERE v.state IN ('published', 'draft');

-- Conflicts of the assignments made before: one full recomputation per
-- tenant (it keeps the conflicts up to date with the load).
SELECT app_queue_load_recompute_all();
