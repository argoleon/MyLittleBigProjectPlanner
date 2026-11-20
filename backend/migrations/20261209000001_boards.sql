-- Kanban boards (specification §6, L06-US05): the tickets of a filter in
-- columns mapped to statuses, with work in progress limits, swimlanes and
-- quick filters.
--
-- * A board belongs to a node of the hierarchy: members who see the node
--   see the board, those with `manage_boards` there create and change it.
--   Every viewer only sees the tickets they may see.
-- * Its filter and quick filters are queries of the language, stored as
--   written (stable keys, never labels); columns name statuses by key, so a
--   board spans the configurations of several projects.
-- * `kind` is `kanban` only; Scrum boards come with sprints (L09).

-- The new permission, in the catalog of roles. Roles that manage sprints
-- (the default "Project manager" and "Node admin") manage boards too.
ALTER TABLE role DROP CONSTRAINT role_permissions_check;
ALTER TABLE role ADD CONSTRAINT role_permissions_check
    CHECK (cardinality(permissions) > 0
           AND permissions <@ ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time',
                                    'assign_resources', 'force_overload', 'manage_sprints',
                                    'manage_boards', 'manage_members', 'administer_node']::text[]);

-- Gives `manage_boards` to the roles of the current tenant that manage
-- sprints; returns how many. The tenant is filtered explicitly: the role
-- running migrations may bypass row-level security.
CREATE FUNCTION app_roles_manage_boards() RETURNS integer
    LANGUAGE sql
    SET search_path = pg_catalog, public
AS $$
    WITH changed AS (
        UPDATE public.role SET permissions = permissions || ARRAY['manage_boards'], version = version + 1
        WHERE tenant_id = public.app_current_tenant()
          AND 'manage_sprints' = ANY (permissions) AND NOT 'manage_boards' = ANY (permissions)
        RETURNING 1
    )
    SELECT count(*)::integer FROM changed
$$;
REVOKE EXECUTE ON FUNCTION app_roles_manage_boards() FROM PUBLIC;

-- Every tenant's roles, one tenant at a time under its own context (RLS).
DO $$
DECLARE
    t record;
BEGIN
    FOR t IN SELECT id FROM tenant LOOP
        PERFORM set_config('app.tenant_id', t.id::text, true);
        PERFORM app_roles_manage_boards();
    END LOOP;
    PERFORM set_config('app.tenant_id', '', true);
END
$$;

CREATE OR REPLACE FUNCTION app_seed_default_roles(p_tenant uuid) RETURNS void
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
                  'force_overload', 'manage_sprints', 'manage_boards']),
        (3, '{"fr": "Administrateur de nœud", "en": "Node admin"}'::jsonb,
            ARRAY['view', 'create_ticket', 'edit', 'comment', 'log_time', 'assign_resources',
                  'force_overload', 'manage_sprints', 'manage_boards', 'manage_members',
                  'administer_node'])
    ) AS r (rank, label, permissions)
$$;
REVOKE EXECUTE ON FUNCTION app_seed_default_roles(uuid) FROM PUBLIC;

CREATE TABLE board (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_id       uuid NOT NULL,
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 100 AND name = btrim(name)),
    kind          text NOT NULL DEFAULT 'kanban' CHECK (kind IN ('kanban')),
    filter        text NOT NULL CHECK (length(filter) <= 10000),
    estimation    text NOT NULL DEFAULT 'points' CHECK (estimation IN ('points', 'hours')),
    swimlanes     text NOT NULL DEFAULT 'none' CHECK (swimlanes IN ('none', 'assignee', 'epic')),
    -- [{"label": …, "query": …}], toggled on the board.
    quick_filters jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(quick_filters) = 'array'),
    created_by    uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    version       bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
-- A name is unique on its node, whatever its case.
CREATE UNIQUE INDEX board_node_name_idx ON board (tenant_id, node_id, lower(name));
SELECT app_setup_business_table('board');

CREATE TABLE board_column (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    board_id   uuid NOT NULL,
    position   integer NOT NULL CHECK (position >= 0),
    name       text NOT NULL CHECK (length(name) BETWEEN 1 AND 40 AND name = btrim(name)),
    -- Keys of the statuses it shows.
    statuses   text[] NOT NULL DEFAULT '{}',
    wip_limit  integer CHECK (wip_limit BETWEEN 1 AND 999),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, board_id, position),
    FOREIGN KEY (tenant_id, board_id) REFERENCES board (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('board_column');
