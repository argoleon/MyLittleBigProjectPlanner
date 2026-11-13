-- Issue types of a project configuration (L04-US02). Specification §5.
--
-- A type belongs to a version of its configuration (L04-US01): the draft
-- copies them from the published version, keeping `item_id`, the identity
-- of a type across versions that tickets will reference (L05). `key` is its
-- readable identifier (`bug`), unique in its version and never changed.
--
-- `parent_ids` lists the types a ticket of this type may sit under, by
-- `item_id`. A ticket of a type that is not a sub-task may also have no
-- parent; a sub-task always has one. The domain refuses cycles
-- (domain::issue_types); the workflow of a type comes with L04-US04.

CREATE TABLE issue_type (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id uuid NOT NULL,
    item_id    uuid NOT NULL,
    key        text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    -- Translatable: {"fr": …, "en": …}, at least one.
    label      jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object' AND label <> '{}'::jsonb),
    -- A Material Symbols name.
    icon       text NOT NULL CHECK (icon ~ '^[a-z0-9_]{1,60}$'),
    -- One of the `--app-type-*` color tokens.
    color      text NOT NULL CHECK (color IN ('epic', 'story', 'task', 'bug')),
    is_subtask boolean NOT NULL DEFAULT false,
    position   integer NOT NULL CHECK (position >= 0),
    parent_ids uuid[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    CONSTRAINT issue_type_key_unique UNIQUE (version_id, key),
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX issue_type_tenant_version_idx ON issue_type (tenant_id, version_id, position);
SELECT app_setup_business_table('issue_type');
-- Seeded with the default configuration, whoever creates the tenant.
CREATE POLICY issue_type_platform ON issue_type TO mlbpp_owner USING (true) WITH CHECK (true);

-- The default types of a version (domain::issue_types::DEFAULT_TYPES; an
-- integration test keeps both in step): Epic at the top; Story, Task and
-- Bug under an Epic or alone; Sub-task under a Story, a Task or a Bug.
CREATE FUNCTION app_seed_default_issue_types(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant uuid;
    epic   uuid := uuidv7();
    story  uuid := uuidv7();
    task   uuid := uuidv7();
    bug    uuid := uuidv7();
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    INSERT INTO public.issue_type (id, tenant_id, version_id, item_id, key, label, icon, color, is_subtask, position, parent_ids)
    VALUES
        (uuidv7(), tenant, p_version, epic, 'epic', '{"fr": "Epic", "en": "Epic"}', 'bolt', 'epic', false, 0, '{}'),
        (uuidv7(), tenant, p_version, story, 'story', '{"fr": "Story", "en": "Story"}', 'bookmark', 'story', false, 1, ARRAY[epic]),
        (uuidv7(), tenant, p_version, task, 'task', '{"fr": "Tâche", "en": "Task"}', 'task_alt', 'task', false, 2, ARRAY[epic]),
        (uuidv7(), tenant, p_version, bug, 'bug', '{"fr": "Bug", "en": "Bug"}', 'bug_report', 'bug', false, 3, ARRAY[epic]),
        (uuidv7(), tenant, p_version, uuidv7(), 'subtask', '{"fr": "Sous-tâche", "en": "Sub-task"}',
         'subdirectory_arrow_right', 'task', true, 4, ARRAY[story, task, bug]);
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
    PERFORM public.app_seed_default_issue_types(version);
    RETURN config;
END
$$;

-- The configurations made before: their published version and draft get
-- the default types.
SELECT app_seed_default_issue_types(v.id)
FROM project_config_version v
WHERE v.state IN ('published', 'draft')
  AND NOT EXISTS (SELECT 1 FROM issue_type t WHERE t.version_id = v.id);
