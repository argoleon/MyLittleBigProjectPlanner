-- Fields of a project configuration (L04-US03). Specification §5.
--
-- Like issue types and workflows, fields belong to a version of their
-- configuration and keep `item_id` from one version to the next: ticket
-- values (L05) are keyed by it.
--
-- - `field_def`: a system or custom field. `kind` is its type, set once;
--   `options` holds what its type takes (domain::fields::Options), list
--   values included, each with an identity it keeps for good and an
--   `active` flag: a published value is deactivated, never removed, so
--   that tickets keep it. A field applies to the types it lists by
--   `item_id`, or to all of them, the ones added later included. System
--   fields (Points, Remaining work) cannot be removed; only their types and
--   rules change.
-- - `field_rule`: one cell of the rules grid, for a type and a step (a
--   status of the type's workflow, or the creation when `status_id` is
--   null). Only Hidden and Required cells are stored: a cell nobody set is
--   Visible, statuses added later included.

CREATE TABLE field_def (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id     uuid NOT NULL,
    item_id        uuid NOT NULL,
    key            text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    -- Translatable: {"fr": …, "en": …}, at least one.
    label          jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object' AND label <> '{}'::jsonb),
    kind           text NOT NULL CHECK (kind IN (
        'short_text', 'rich_text', 'number', 'checkbox', 'date', 'date_time', 'single_select',
        'multi_select', 'cascading_select', 'user', 'group', 'labels', 'url', 'issue_link'
    )),
    options        jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(options) = 'object'),
    is_system      boolean NOT NULL DEFAULT false,
    all_types      boolean NOT NULL DEFAULT false,
    issue_type_ids uuid[] NOT NULL DEFAULT '{}',
    position       integer NOT NULL CHECK (position >= 0),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1,
    CONSTRAINT field_def_key_unique UNIQUE (version_id, key),
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX field_def_tenant_version_idx ON field_def (tenant_id, version_id, position);
SELECT app_setup_business_table('field_def');
-- Seeded with the default configuration, whoever creates the tenant.
CREATE POLICY field_def_platform ON field_def TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE field_rule (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id    uuid NOT NULL,
    field_id      uuid NOT NULL,
    issue_type_id uuid NOT NULL,
    -- Null: at creation.
    status_id     uuid,
    mode          text NOT NULL CHECK (mode IN ('hidden', 'required')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT field_rule_cell_unique UNIQUE NULLS NOT DISTINCT (version_id, field_id, issue_type_id, status_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (version_id, field_id) REFERENCES field_def (version_id, item_id) ON DELETE CASCADE,
    -- A type or a status removed takes its cells with it.
    FOREIGN KEY (version_id, issue_type_id) REFERENCES issue_type (version_id, item_id) ON DELETE CASCADE,
    FOREIGN KEY (version_id, status_id) REFERENCES workflow_status (version_id, item_id) ON DELETE CASCADE
);
CREATE INDEX field_rule_tenant_version_idx ON field_rule (tenant_id, version_id, field_id);
SELECT app_setup_business_table('field_rule');
CREATE POLICY field_rule_platform ON field_rule TO mlbpp_owner USING (true) WITH CHECK (true);

-- The system fields of a version (domain::fields::SYSTEM_FIELDS; an
-- integration test keeps both in step): Points on stories and tasks,
-- Remaining work on every type. Seeded after the types.
CREATE FUNCTION app_seed_default_fields(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant uuid;
    points uuid[];
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    SELECT coalesce(array_agg(item_id ORDER BY position), '{}') INTO points
    FROM public.issue_type WHERE version_id = p_version AND key IN ('story', 'task');
    INSERT INTO public.field_def
        (id, tenant_id, version_id, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position)
    VALUES
        (uuidv7(), tenant, p_version, uuidv7(), 'story_points', '{"fr": "Points", "en": "Points"}', 'number',
         '{"decimals": 1}', true, false, points, 0),
        (uuidv7(), tenant, p_version, uuidv7(), 'remaining', '{"fr": "Reste à faire", "en": "Remaining work"}', 'number',
         '{"decimals": 2, "unit": "hours"}', true, true, '{}', 1);
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
    -- The workflow first: the types follow it; then the fields, on the types.
    PERFORM public.app_seed_default_workflow(version);
    PERFORM public.app_seed_default_issue_types(version);
    PERFORM public.app_seed_default_fields(version);
    RETURN config;
END
$$;

-- The published versions made before get the system fields; their drafts
-- copy them, with the same identities, so that publishing them changes
-- nothing.
SELECT app_seed_default_fields(v.id)
FROM project_config_version v
WHERE v.state = 'published'
  AND NOT EXISTS (SELECT 1 FROM field_def f WHERE f.version_id = v.id);

INSERT INTO field_def
    (id, tenant_id, version_id, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position)
SELECT uuidv7(), f.tenant_id, d.id, f.item_id, f.key, f.label, f.kind, f.options, f.is_system, f.all_types,
       coalesce(ARRAY(SELECT t.item_id FROM issue_type t
                      WHERE t.version_id = d.id AND t.item_id = ANY (f.issue_type_ids)
                      ORDER BY t.position), '{}'),
       f.position
FROM project_config_version d
JOIN project_config_version p ON p.config_id = d.config_id AND p.state = 'published'
JOIN field_def f ON f.version_id = p.id
WHERE d.state = 'draft'
  AND NOT EXISTS (SELECT 1 FROM field_def x WHERE x.version_id = d.id);
