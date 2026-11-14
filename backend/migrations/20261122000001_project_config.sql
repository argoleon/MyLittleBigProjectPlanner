-- Project configurations (L04-US01). Specification §5, §18.
--
-- - `project_config`: a configuration, shared by any number of projects. Each
--   tenant has exactly one default configuration, which new projects get.
-- - `project_config_version`: what a configuration holds lives in versions.
--   A configuration has one published version, which projects and tickets
--   use, and at most one draft, where its admins edit; publishing swaps them
--   in one transaction and retires the former published version. The
--   content tables of the later stories (types, fields, workflows, screens,
--   notifications) belong to a version.
-- - `project_config_change`: what the draft changed, one row per changed
--   item, whatever the number of edits: the "2 changes" of the draft chip.
-- - `node.config_id`: the configuration of a project, exactly one.

CREATE TABLE project_config (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- Translatable: {"fr": …, "en": …}, at least one.
    name       jsonb NOT NULL CHECK (jsonb_typeof(name) = 'object' AND name <> '{}'::jsonb),
    is_default boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    -- Target of the tenant-checked foreign keys below.
    UNIQUE (tenant_id, id)
);
-- A name names one configuration per tenant and language, whatever its case.
CREATE UNIQUE INDEX project_config_tenant_name_fr_idx ON project_config (tenant_id, lower(name ->> 'fr'));
CREATE UNIQUE INDEX project_config_tenant_name_en_idx ON project_config (tenant_id, lower(name ->> 'en'));
CREATE UNIQUE INDEX project_config_tenant_default_idx ON project_config (tenant_id) WHERE is_default;
CREATE INDEX project_config_tenant_created_idx ON project_config (tenant_id, created_at, id);
SELECT app_setup_business_table('project_config');
-- Seeded by the function below, whoever creates the tenant.
CREATE POLICY project_config_platform ON project_config TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE project_config_version (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    config_id    uuid NOT NULL,
    -- 1 for the first published version, then one more per draft.
    number       integer NOT NULL CHECK (number >= 1),
    state        text NOT NULL CHECK (state IN ('draft', 'published', 'retired')),
    created_by   uuid REFERENCES account (id) ON DELETE SET NULL,
    published_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    CONSTRAINT project_config_version_published_at CHECK ((state = 'draft') = (published_at IS NULL)),
    UNIQUE (tenant_id, id),
    UNIQUE (config_id, number),
    FOREIGN KEY (tenant_id, config_id) REFERENCES project_config (tenant_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX project_config_version_one_draft_idx
    ON project_config_version (config_id) WHERE state = 'draft';
CREATE UNIQUE INDEX project_config_version_one_published_idx
    ON project_config_version (config_id) WHERE state = 'published';
CREATE INDEX project_config_version_tenant_config_idx ON project_config_version (tenant_id, config_id);
SELECT app_setup_business_table('project_config_version');
CREATE POLICY project_config_version_platform ON project_config_version TO mlbpp_owner
    USING (true) WITH CHECK (true);

CREATE TABLE project_config_change (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id uuid NOT NULL,
    -- What changed: its kind and its identifier, stable across versions.
    item_kind  text NOT NULL CHECK (item_kind IN ('issue_type', 'field', 'workflow', 'screen', 'notification')),
    item_id    uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    UNIQUE (version_id, item_kind, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX project_config_change_tenant_version_idx ON project_config_change (tenant_id, version_id);
SELECT app_setup_business_table('project_config_change');

-- The default configuration of a tenant, published, version 1. Its content
-- (types, fields, workflow) comes with L04-US02 to US04.
CREATE FUNCTION app_seed_default_config(p_tenant uuid) RETURNS uuid
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    config uuid := uuidv7();
BEGIN
    INSERT INTO public.project_config (id, tenant_id, name, is_default)
    VALUES (config, p_tenant, '{"fr": "Configuration par défaut", "en": "Default configuration"}', true);
    INSERT INTO public.project_config_version (id, tenant_id, config_id, number, state, published_at)
    VALUES (uuidv7(), p_tenant, config, 1, 'published', now());
    RETURN config;
END
$$;
REVOKE EXECUTE ON FUNCTION app_seed_default_config(uuid) FROM PUBLIC;

CREATE OR REPLACE FUNCTION app_create_tenant_defaults() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    -- A UUID v7 like every key: 48-bit milliseconds, version 7, variant 2.
    INSERT INTO public.tenant_auth_policy (id, tenant_id) VALUES (uuidv7(), NEW.id);
    PERFORM public.app_seed_default_roles(NEW.id);
    PERFORM public.app_seed_default_config(NEW.id);
    RETURN NEW;
END
$$;

SELECT app_seed_default_config(id) FROM tenant;

-- The configuration of a project: exactly one, the tenant's default unless
-- given. Portfolios and programs have none.
ALTER TABLE node ADD COLUMN config_id uuid;
ALTER TABLE node ADD FOREIGN KEY (tenant_id, config_id) REFERENCES project_config (tenant_id, id);
CREATE INDEX node_tenant_config_idx ON node (tenant_id, config_id) WHERE config_id IS NOT NULL;

UPDATE node SET config_id = c.id
FROM project_config c
WHERE c.tenant_id = node.tenant_id AND c.is_default AND node.node_type = 'project';

ALTER TABLE node ADD CONSTRAINT node_config_of_projects
    CHECK ((node_type = 'project') = (config_id IS NOT NULL));

CREATE FUNCTION app_default_node_config() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.node_type = 'project' AND NEW.config_id IS NULL THEN
        SELECT id INTO NEW.config_id FROM project_config
        WHERE tenant_id = NEW.tenant_id AND is_default;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER default_node_config BEFORE INSERT ON node
    FOR EACH ROW EXECUTE FUNCTION app_default_node_config();
