-- Notification rules of a project configuration (L04-US07). Specification
-- §5 and §12.
--
-- One rule per ticket event and version, always there (no recipient: the
-- event notifies nobody). Its recipients: people of the ticket (reporter,
-- assignee, watchers, the people mentioned), roles of the tenant (their
-- holders on the ticket's project, inherited from above), groups and
-- members. The author of a change is never notified of it
-- (domain::notifications). Each member's own preferences come with L05.

CREATE TABLE notification_rule (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id      uuid NOT NULL,
    event           text NOT NULL CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned')),
    recipient_kinds text[] NOT NULL DEFAULT '{}'
        CHECK (recipient_kinds <@ ARRAY['reporter', 'assignee', 'watchers', 'mentioned']),
    role_ids        uuid[] NOT NULL DEFAULT '{}',
    group_ids       uuid[] NOT NULL DEFAULT '{}',
    account_ids     uuid[] NOT NULL DEFAULT '{}',
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    version         bigint NOT NULL DEFAULT 1,
    CONSTRAINT notification_rule_event_unique UNIQUE (version_id, event),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX notification_rule_tenant_version_idx ON notification_rule (tenant_id, version_id);
SELECT app_setup_business_table('notification_rule');
CREATE POLICY notification_rule_platform ON notification_rule TO mlbpp_owner USING (true) WITH CHECK (true);

-- The default rules of a version (domain::notifications::DEFAULT_RULES; an
-- integration test keeps both in step), as Jira's default scheme.
CREATE FUNCTION app_seed_default_notifications(p_version uuid) RETURNS void
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
END
$$;

CREATE OR REPLACE FUNCTION app_seed_default_config(p_tenant uuid) RETURNS uuid
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    config  uuid := uuidv7();
    version uuid := uuidv7();
BEGIN
    INSERT INTO public.project_config (id, tenant_id, name, is_default)
    VALUES (config, p_tenant, '{"fr": "Configuration par défaut", "en": "Default configuration"}', true);
    INSERT INTO public.project_config_version (id, tenant_id, config_id, number, state, published_at)
    VALUES (version, p_tenant, config, 1, 'published', now());
    -- The workflow first: the types follow it; then the fields, on the
    -- types; then the screens, of the fields; then the notification rules.
    PERFORM public.app_seed_default_workflow(version);
    PERFORM public.app_seed_default_issue_types(version);
    PERFORM public.app_seed_default_fields(version);
    PERFORM public.app_seed_default_screens(version);
    PERFORM public.app_seed_default_notifications(version);
    RETURN config;
END
$$;

-- The versions made before (their drafts too: rules have no identity of
-- their own beyond their event).
SELECT app_seed_default_notifications(v.id)
FROM project_config_version v
WHERE v.state IN ('published', 'draft')
  AND NOT EXISTS (SELECT 1 FROM notification_rule n WHERE n.version_id = v.id);
