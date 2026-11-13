-- Aggregates of the hierarchy (L03-US05). Specification §4.
--
-- - `project_figure`: what is measured on a project by itself (planned
--   load, capacity of the teams assigned, tickets by status category, late
--   tickets). Tickets (L05) and workload (L08) write it; a project without a
--   row counts for nothing.
-- - `node_aggregate`: the figures of each node over its distinct descendant
--   projects, precomputed by the worker (`domain::aggregates`).
-- - `node_aggregate_stale`: the nodes whose aggregates, and those of their
--   ancestors, are to be recomputed. A change marks them in its own
--   transaction and queues a `node.aggregates` job; the job takes every mark
--   at once.

CREATE TABLE project_figure (
    tenant_id           uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- A project (the repository checks the type).
    node_id             uuid NOT NULL,
    planned_minutes     bigint NOT NULL DEFAULT 0 CHECK (planned_minutes >= 0),
    capacity_minutes    bigint NOT NULL DEFAULT 0 CHECK (capacity_minutes >= 0),
    tickets_todo        bigint NOT NULL DEFAULT 0 CHECK (tickets_todo >= 0),
    tickets_in_progress bigint NOT NULL DEFAULT 0 CHECK (tickets_in_progress >= 0),
    tickets_done        bigint NOT NULL DEFAULT 0 CHECK (tickets_done >= 0),
    -- Open tickets past their due date.
    tickets_late        bigint NOT NULL DEFAULT 0 CHECK (tickets_late >= 0),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    version             bigint NOT NULL DEFAULT 1,
    PRIMARY KEY (tenant_id, node_id),
    CONSTRAINT project_figure_late_are_open CHECK (tickets_late <= tickets_todo + tickets_in_progress),
    FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('project_figure');
CREATE POLICY project_figure_platform ON project_figure TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE node_aggregate (
    tenant_id                 uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_id                   uuid NOT NULL,
    -- Distinct projects counted, and those of them that are confidential.
    projects                  integer NOT NULL DEFAULT 0 CHECK (projects >= 0),
    confidential_projects     integer NOT NULL DEFAULT 0 CHECK (confidential_projects >= 0),
    -- Amounts, weighted by the links of the node's direct children.
    planned_minutes           bigint NOT NULL DEFAULT 0 CHECK (planned_minutes >= 0),
    capacity_minutes          bigint NOT NULL DEFAULT 0 CHECK (capacity_minutes >= 0),
    -- Tickets, whole.
    tickets_todo              bigint NOT NULL DEFAULT 0 CHECK (tickets_todo >= 0),
    tickets_in_progress       bigint NOT NULL DEFAULT 0 CHECK (tickets_in_progress >= 0),
    tickets_done              bigint NOT NULL DEFAULT 0 CHECK (tickets_done >= 0),
    tickets_late              bigint NOT NULL DEFAULT 0 CHECK (tickets_late >= 0),
    -- Open tickets of the confidential projects.
    confidential_open_tickets bigint NOT NULL DEFAULT 0 CHECK (confidential_open_tickets >= 0),
    created_at                timestamptz NOT NULL DEFAULT now(),
    updated_at                timestamptz NOT NULL DEFAULT now(),
    version                   bigint NOT NULL DEFAULT 1,
    PRIMARY KEY (tenant_id, node_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('node_aggregate');
CREATE POLICY node_aggregate_platform ON node_aggregate TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE TABLE node_aggregate_stale (
    tenant_id uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_id   uuid NOT NULL,
    PRIMARY KEY (tenant_id, node_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
ALTER TABLE node_aggregate_stale ENABLE ROW LEVEL SECURITY;
ALTER TABLE node_aggregate_stale FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON node_aggregate_stale
    USING (tenant_id = app_current_tenant()) WITH CHECK (tenant_id = app_current_tenant());
CREATE POLICY node_aggregate_stale_platform ON node_aggregate_stale TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, DELETE ON node_aggregate_stale TO mlbpp_app;

-- Nodes created before this migration: all to compute, one job per tenant.
INSERT INTO node_aggregate_stale (tenant_id, node_id)
SELECT tenant_id, id FROM node;
INSERT INTO job (id, tenant_id, kind)
SELECT uuidv7(), tenant_id, 'node.aggregates'
FROM (SELECT DISTINCT tenant_id FROM node) AS tenants;
