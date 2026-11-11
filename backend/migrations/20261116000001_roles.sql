-- Node roles as sets of permissions (L02-US07). Specification §14.
--
-- Each tenant defines its roles; L03 grants them to members and groups on
-- hierarchy nodes. Permissions come from the product's fixed catalog
-- (domain::permissions::Permission).

CREATE TABLE role (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- Translatable: {"fr": …, "en": …}, at least one.
    label       jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object' AND label <> '{}'::jsonb),
    permissions text[] NOT NULL
                CHECK (cardinality(permissions) > 0
                       AND permissions <@ ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time',
                                                'assign_resources', 'force_overload', 'manage_sprints',
                                                'manage_members', 'administer_node']::text[]),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1
);
-- A label names one role per tenant and language, whatever its case.
CREATE UNIQUE INDEX role_tenant_label_fr_idx ON role (tenant_id, lower(label ->> 'fr'));
CREATE UNIQUE INDEX role_tenant_label_en_idx ON role (tenant_id, lower(label ->> 'en'));
CREATE INDEX role_tenant_created_idx ON role (tenant_id, created_at, id);
SELECT app_setup_business_table('role');
-- Seeded by the function below, whoever creates the tenant.
CREATE POLICY role_platform ON role TO mlbpp_owner USING (true) WITH CHECK (true);

-- The default roles of a new tenant (domain::permissions::DEFAULT_ROLES;
-- an integration test keeps both in step). Created one millisecond apart,
-- so that they list from the narrowest to the widest.
CREATE FUNCTION app_seed_default_roles(p_tenant uuid) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    INSERT INTO public.role (id, tenant_id, label, permissions, created_at)
    SELECT uuidv7(), p_tenant, r.label, r.permissions, now() + r.rank * interval '1 millisecond'
    FROM (VALUES
        (0, '{"fr": "Lecteur", "en": "Reader"}'::jsonb, ARRAY['view']),
        (1, '{"fr": "Contributeur", "en": "Contributor"}'::jsonb,
            ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time']),
        (2, '{"fr": "Chef de projet", "en": "Project manager"}'::jsonb,
            ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time', 'assign_resources',
                  'force_overload', 'manage_sprints']),
        (3, '{"fr": "Administrateur de nœud", "en": "Node admin"}'::jsonb,
            ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time', 'assign_resources',
                  'force_overload', 'manage_sprints', 'manage_members', 'administer_node'])
    ) AS r (rank, label, permissions)
$$;
REVOKE EXECUTE ON FUNCTION app_seed_default_roles(uuid) FROM PUBLIC;

CREATE OR REPLACE FUNCTION app_create_tenant_defaults() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    -- A UUID v7 like every key: 48-bit milliseconds, version 7, variant 2.
    INSERT INTO public.tenant_auth_policy (id, tenant_id) VALUES (uuidv7(), NEW.id);
    PERFORM public.app_seed_default_roles(NEW.id);
    RETURN NEW;
END
$$;

SELECT app_seed_default_roles(id) FROM tenant;
