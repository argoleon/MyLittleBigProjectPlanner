-- Workflows of a project configuration (L04-US04). Specification §5.
--
-- Like issue types, workflows belong to a version of their configuration
-- and keep `item_id` from one version to the next; so do their statuses
-- and transitions.
--
-- - `workflow`: a lifecycle, which any number of issue types follow
--   (`issue_type.workflow_id`); each type follows exactly one.
-- - `workflow_status`: its statuses, each in one category (To do, In
--   progress, Done) that aggregates, boards and burndown read whatever the
--   labels; one is the initial status, and a label may differ on the
--   portal (L13).
-- - `workflow_transition`: a named move to a status, from one status or
--   from any (`from_status_id` null). Conditions, validators and
--   post-functions come with L04-US05, the transition screen with US06.
--
-- A draft may hold an incomplete workflow (no initial status, a status
-- nothing leads to): domain::workflows lists the problems, and publishing
-- refuses them.

CREATE TABLE workflow (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id uuid NOT NULL,
    item_id    uuid NOT NULL,
    key        text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    -- Translatable: {"fr": …, "en": …}, at least one.
    label      jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object' AND label <> '{}'::jsonb),
    position   integer NOT NULL CHECK (position >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    CONSTRAINT workflow_key_unique UNIQUE (version_id, key),
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX workflow_tenant_version_idx ON workflow (tenant_id, version_id, position);
SELECT app_setup_business_table('workflow');
CREATE POLICY workflow_platform ON workflow TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE workflow_status (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id   uuid NOT NULL,
    item_id      uuid NOT NULL,
    workflow_id  uuid NOT NULL,
    key          text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    label        jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object' AND label <> '{}'::jsonb),
    -- What requesters see on the portal; empty: the label.
    portal_label jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(portal_label) = 'object'),
    category     text NOT NULL CHECK (category IN ('todo', 'in_progress', 'done')),
    is_initial   boolean NOT NULL DEFAULT false,
    position     integer NOT NULL CHECK (position >= 0),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    CONSTRAINT workflow_status_key_unique UNIQUE (version_id, workflow_id, key),
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (version_id, workflow_id) REFERENCES workflow (version_id, item_id) ON DELETE CASCADE
);
-- At most one initial status per workflow; none is a problem for publishing.
CREATE UNIQUE INDEX workflow_status_one_initial_idx ON workflow_status (version_id, workflow_id) WHERE is_initial;
CREATE INDEX workflow_status_tenant_version_idx ON workflow_status (tenant_id, version_id, workflow_id, position);
SELECT app_setup_business_table('workflow_status');
CREATE POLICY workflow_status_platform ON workflow_status TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE workflow_transition (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id     uuid NOT NULL,
    item_id        uuid NOT NULL,
    workflow_id    uuid NOT NULL,
    name           jsonb NOT NULL CHECK (jsonb_typeof(name) = 'object' AND name <> '{}'::jsonb),
    -- Null: from any status of the workflow.
    from_status_id uuid,
    to_status_id   uuid NOT NULL,
    position       integer NOT NULL CHECK (position >= 0),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1,
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (version_id, workflow_id) REFERENCES workflow (version_id, item_id) ON DELETE CASCADE,
    -- A status removed takes its transitions with it.
    FOREIGN KEY (version_id, from_status_id) REFERENCES workflow_status (version_id, item_id) ON DELETE CASCADE,
    FOREIGN KEY (version_id, to_status_id) REFERENCES workflow_status (version_id, item_id) ON DELETE CASCADE
);
CREATE INDEX workflow_transition_tenant_version_idx ON workflow_transition (tenant_id, version_id, workflow_id, position);
SELECT app_setup_business_table('workflow_transition');
CREATE POLICY workflow_transition_platform ON workflow_transition TO mlbpp_owner USING (true) WITH CHECK (true);

-- Each issue type follows one workflow of its version (L04-US02 left it out).
ALTER TABLE issue_type ADD COLUMN workflow_id uuid;
ALTER TABLE issue_type ADD FOREIGN KEY (version_id, workflow_id) REFERENCES workflow (version_id, item_id);

-- The standard workflow of mockup 11 (domain::workflows::STANDARD; an
-- integration test keeps both in step), followed by the types of the
-- version that have none.
CREATE FUNCTION app_seed_default_workflow(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant      uuid;
    standard    uuid := uuidv7();
    todo        uuid := uuidv7();
    in_progress uuid := uuidv7();
    in_review   uuid := uuidv7();
    done        uuid := uuidv7();
    cancelled   uuid := uuidv7();
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    INSERT INTO public.workflow (id, tenant_id, version_id, item_id, key, label, position)
    VALUES (uuidv7(), tenant, p_version, standard, 'standard', '{"fr": "Workflow standard", "en": "Standard workflow"}', 0);
    INSERT INTO public.workflow_status
        (id, tenant_id, version_id, item_id, workflow_id, key, label, portal_label, category, is_initial, position)
    VALUES
        (uuidv7(), tenant, p_version, todo, standard, 'todo', '{"fr": "À faire", "en": "To do"}',
         '{"fr": "Reçue", "en": "Received"}', 'todo', true, 0),
        (uuidv7(), tenant, p_version, in_progress, standard, 'in_progress', '{"fr": "En cours", "en": "In progress"}',
         '{"fr": "En cours de traitement", "en": "Being processed"}', 'in_progress', false, 1),
        (uuidv7(), tenant, p_version, in_review, standard, 'in_review', '{"fr": "En revue", "en": "In review"}',
         '{"fr": "En cours de traitement", "en": "Being processed"}', 'in_progress', false, 2),
        (uuidv7(), tenant, p_version, done, standard, 'done', '{"fr": "Terminé", "en": "Done"}',
         '{"fr": "Résolue", "en": "Resolved"}', 'done', false, 3),
        (uuidv7(), tenant, p_version, cancelled, standard, 'cancelled', '{"fr": "Annulé", "en": "Cancelled"}',
         '{"fr": "Fermée", "en": "Closed"}', 'done', false, 4);
    INSERT INTO public.workflow_transition
        (id, tenant_id, version_id, item_id, workflow_id, name, from_status_id, to_status_id, position)
    VALUES
        (uuidv7(), tenant, p_version, uuidv7(), standard, '{"fr": "Démarrer", "en": "Start"}', todo, in_progress, 0),
        (uuidv7(), tenant, p_version, uuidv7(), standard, '{"fr": "Passer en revue", "en": "Send to review"}',
         in_progress, in_review, 1),
        (uuidv7(), tenant, p_version, uuidv7(), standard, '{"fr": "Refuser", "en": "Reject"}', in_review, in_progress, 2),
        (uuidv7(), tenant, p_version, uuidv7(), standard, '{"fr": "Valider", "en": "Validate"}', in_review, done, 3),
        (uuidv7(), tenant, p_version, uuidv7(), standard, '{"fr": "Annuler", "en": "Cancel"}', NULL, cancelled, 4);
    UPDATE public.issue_type SET workflow_id = standard WHERE version_id = p_version AND workflow_id IS NULL;
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
    -- The workflow first: the types follow it.
    PERFORM public.app_seed_default_workflow(version);
    PERFORM public.app_seed_default_issue_types(version);
    RETURN config;
END
$$;

-- The default types now follow the first workflow of their version.
CREATE OR REPLACE FUNCTION app_seed_default_issue_types(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant   uuid;
    workflow uuid;
    epic     uuid := uuidv7();
    story    uuid := uuidv7();
    task     uuid := uuidv7();
    bug      uuid := uuidv7();
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    SELECT item_id INTO workflow FROM public.workflow WHERE version_id = p_version ORDER BY position, key LIMIT 1;
    INSERT INTO public.issue_type
        (id, tenant_id, version_id, item_id, key, label, icon, color, is_subtask, position, parent_ids, workflow_id)
    VALUES
        (uuidv7(), tenant, p_version, epic, 'epic', '{"fr": "Epic", "en": "Epic"}', 'bolt', 'epic', false, 0, '{}', workflow),
        (uuidv7(), tenant, p_version, story, 'story', '{"fr": "Story", "en": "Story"}', 'bookmark', 'story', false, 1,
         ARRAY[epic], workflow),
        (uuidv7(), tenant, p_version, task, 'task', '{"fr": "Tâche", "en": "Task"}', 'task_alt', 'task', false, 2,
         ARRAY[epic], workflow),
        (uuidv7(), tenant, p_version, bug, 'bug', '{"fr": "Bug", "en": "Bug"}', 'bug_report', 'bug', false, 3,
         ARRAY[epic], workflow),
        (uuidv7(), tenant, p_version, uuidv7(), 'subtask', '{"fr": "Sous-tâche", "en": "Sub-task"}',
         'subdirectory_arrow_right', 'task', true, 4, ARRAY[story, task, bug], workflow);
END
$$;

-- The versions made before: the standard workflow, followed by their types.
SELECT app_seed_default_workflow(v.id)
FROM project_config_version v
WHERE v.state IN ('published', 'draft')
  AND NOT EXISTS (SELECT 1 FROM workflow w WHERE w.version_id = v.id);

-- Retired versions keep their types without a workflow; the others have one.
ALTER TABLE issue_type ADD CONSTRAINT issue_type_has_workflow CHECK (workflow_id IS NOT NULL) NOT VALID;
