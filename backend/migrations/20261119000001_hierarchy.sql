-- Portfolio, program and project hierarchy (L03-US01). Specification §4,
-- §18.
--
-- - `node`: portfolios, programs and projects, with their description,
--   owners (`node_owner`), dates, health and objectives; a project has a key
--   unique in its tenant (`PLAT`).
-- - `node_edge`: parent-child links, with the weighting of the parent's
--   aggregates. A node has any number of parents within the attachment
--   matrix, and never becomes its own ancestor.
-- - `node_closure`: every (ancestor, descendant) pair with its number of
--   paths, each node being its own ancestor at depth 0. Triggers keep it in
--   step with `node_edge`; it answers descendant lookups, the inheritance of
--   permissions and the cycle check at once.

CREATE TABLE node (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_type    text NOT NULL CHECK (node_type IN ('portfolio', 'program', 'project')),
    name         text NOT NULL CHECK (length(name) BETWEEN 1 AND 200 AND name = btrim(name)),
    -- Projects only: 2 to 10 capital letters and digits, a letter first.
    key          text CHECK (key ~ '^[A-Z][A-Z0-9]{1,9}$'),
    description  text NOT NULL DEFAULT '' CHECK (length(description) <= 10000),
    objectives   text NOT NULL DEFAULT '' CHECK (length(objectives) <= 10000),
    -- Planning dates: local, never converted through UTC.
    start_date   date,
    end_date     date,
    health       text NOT NULL DEFAULT 'on_track' CHECK (health IN ('on_track', 'at_risk', 'off_track')),
    -- Archiving (L03-US06) and confidentiality (L03-US04).
    archived_at  timestamptz,
    confidential boolean NOT NULL DEFAULT false,
    created_by   uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    CONSTRAINT node_key_of_projects CHECK ((node_type = 'project') = (key IS NOT NULL)),
    CONSTRAINT node_dates_in_order CHECK (end_date IS NULL OR start_date IS NULL OR end_date >= start_date),
    -- Target of the tenant-checked foreign keys below.
    UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX node_tenant_key_idx ON node (tenant_id, key);
CREATE INDEX node_tenant_created_idx ON node (tenant_id, created_at, id);
CREATE INDEX node_tenant_type_idx ON node (tenant_id, node_type, created_at, id);
SELECT app_setup_business_table('node');
CREATE POLICY node_platform ON node TO mlbpp_owner USING (true) WITH CHECK (true);

-- A node keeps its type: the matrix of its links depends on it.
CREATE FUNCTION app_keep_node_type() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.node_type <> OLD.node_type THEN
        RAISE EXCEPTION 'node.type_fixed' USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER keep_node_type BEFORE UPDATE OF node_type ON node
    FOR EACH ROW EXECUTE FUNCTION app_keep_node_type();

-- Owners: members of the tenant (they leave with their membership).
CREATE TABLE node_owner (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_id    uuid NOT NULL,
    account_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, node_id, account_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, account_id) REFERENCES membership (tenant_id, account_id) ON DELETE CASCADE
);
SELECT app_setup_business_table('node_owner');
CREATE POLICY node_owner_platform ON node_owner TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE node_edge (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    parent_id      uuid NOT NULL,
    child_id       uuid NOT NULL,
    -- Share of the child in the parent's aggregates (budget allocations).
    weight_percent smallint NOT NULL DEFAULT 100 CHECK (weight_percent BETWEEN 0 AND 100),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1,
    CHECK (parent_id <> child_id),
    UNIQUE (tenant_id, parent_id, child_id),
    FOREIGN KEY (tenant_id, parent_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, child_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX node_edge_tenant_child_idx ON node_edge (tenant_id, child_id);
SELECT app_setup_business_table('node_edge');
CREATE POLICY node_edge_platform ON node_edge TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE node_closure (
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    ancestor_id   uuid NOT NULL,
    descendant_id uuid NOT NULL,
    -- Distinct paths from the ancestor down to the descendant (1 for a node
    -- and itself).
    path_count    bigint NOT NULL CHECK (path_count > 0),
    PRIMARY KEY (tenant_id, ancestor_id, descendant_id),
    FOREIGN KEY (tenant_id, ancestor_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, descendant_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX node_closure_tenant_descendant_idx ON node_closure (tenant_id, descendant_id, ancestor_id);
ALTER TABLE node_closure ENABLE ROW LEVEL SECURITY;
ALTER TABLE node_closure FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON node_closure
    USING (tenant_id = app_current_tenant()) WITH CHECK (tenant_id = app_current_tenant());
CREATE POLICY node_closure_platform ON node_closure TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, UPDATE, DELETE ON node_closure TO mlbpp_app;

-- Each node is its own ancestor.
CREATE FUNCTION app_node_closure_self() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO node_closure (tenant_id, ancestor_id, descendant_id, path_count)
    VALUES (NEW.tenant_id, NEW.id, NEW.id, 1);
    RETURN NEW;
END
$$;
CREATE TRIGGER node_closure_self AFTER INSERT ON node
    FOR EACH ROW EXECUTE FUNCTION app_node_closure_self();

-- Before a link: the attachment matrix, and no cycle. Links of a tenant are
-- changed one transaction at a time, so that two links made at once cannot
-- close a cycle together.
CREATE FUNCTION app_check_node_edge() RETURNS trigger
    LANGUAGE plpgsql
AS $$
DECLARE
    v_parent text;
    v_child  text;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('node_edge:' || NEW.tenant_id::text, 0));
    SELECT node_type INTO v_parent FROM node WHERE tenant_id = NEW.tenant_id AND id = NEW.parent_id;
    SELECT node_type INTO v_child FROM node WHERE tenant_id = NEW.tenant_id AND id = NEW.child_id;
    -- Child type: allowed parents (specification §4).
    IF NOT (
        (v_child = 'portfolio' AND v_parent = 'portfolio')
        OR (v_child = 'program' AND v_parent IN ('portfolio', 'program'))
        OR (v_child = 'project' AND v_parent IN ('portfolio', 'program', 'project'))
    ) THEN
        RAISE EXCEPTION 'node.parent_not_allowed' USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (
        SELECT 1 FROM node_closure
        WHERE tenant_id = NEW.tenant_id AND ancestor_id = NEW.child_id AND descendant_id = NEW.parent_id
    ) THEN
        RAISE EXCEPTION 'node.cycle' USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER check_node_edge BEFORE INSERT OR UPDATE OF parent_id, child_id ON node_edge
    FOR EACH ROW EXECUTE FUNCTION app_check_node_edge();

-- A new link adds, for every ancestor of the parent and every descendant of
-- the child, the product of their path counts.
CREATE FUNCTION app_node_closure_link() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO node_closure (tenant_id, ancestor_id, descendant_id, path_count)
    SELECT NEW.tenant_id, a.ancestor_id, d.descendant_id, sum(a.path_count * d.path_count)
    FROM node_closure a
    JOIN node_closure d ON d.tenant_id = a.tenant_id
    WHERE a.tenant_id = NEW.tenant_id AND a.descendant_id = NEW.parent_id AND d.ancestor_id = NEW.child_id
    GROUP BY a.ancestor_id, d.descendant_id
    ON CONFLICT (tenant_id, ancestor_id, descendant_id)
        DO UPDATE SET path_count = node_closure.path_count + EXCLUDED.path_count;
    RETURN NEW;
END
$$;
CREATE TRIGGER node_closure_link AFTER INSERT ON node_edge
    FOR EACH ROW EXECUTE FUNCTION app_node_closure_link();

-- A removed link takes the same paths away; pairs left without path go.
-- One statement: the removed paths are counted once, on the closure as it
-- was before.
CREATE FUNCTION app_node_closure_unlink() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('node_edge:' || OLD.tenant_id::text, 0));
    WITH removed AS (
        SELECT a.ancestor_id, d.descendant_id, sum(a.path_count * d.path_count) AS paths
        FROM node_closure a
        JOIN node_closure d ON d.tenant_id = a.tenant_id
        WHERE a.tenant_id = OLD.tenant_id AND a.descendant_id = OLD.parent_id AND d.ancestor_id = OLD.child_id
        GROUP BY a.ancestor_id, d.descendant_id
    ), reduced AS (
        UPDATE node_closure c
        SET path_count = c.path_count - removed.paths
        FROM removed
        WHERE c.tenant_id = OLD.tenant_id
          AND c.ancestor_id = removed.ancestor_id AND c.descendant_id = removed.descendant_id
          AND c.path_count > removed.paths
        RETURNING 1
    )
    DELETE FROM node_closure c
    USING removed
    WHERE c.tenant_id = OLD.tenant_id
      AND c.ancestor_id = removed.ancestor_id AND c.descendant_id = removed.descendant_id
      AND c.path_count <= removed.paths;
    RETURN OLD;
END
$$;
CREATE TRIGGER node_closure_unlink AFTER DELETE ON node_edge
    FOR EACH ROW EXECUTE FUNCTION app_node_closure_unlink();
