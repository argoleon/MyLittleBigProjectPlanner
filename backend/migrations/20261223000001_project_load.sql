-- Load against capacity by project, program and portfolio (specification
-- §4, §8, L08-US07).
--
-- `load_aggregate_project`: per resource, week or month, and project, the
-- resource's load on the project's tickets and its share of the resource's
-- capacity, in proportion to that load (all shares of a resource add up to
-- its capacity). Recomputed with the resource's sums (infra::load_aggregates)
-- over the same horizon. A node sums the shares of its distinct descendant
-- projects with the weighting of its links (domain::aggregates); a project's
-- totals feed `project_figure`, which the hierarchy's aggregates sum up.

CREATE TABLE load_aggregate_project (
    tenant_id           uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id         uuid NOT NULL,
    period_kind         text NOT NULL CHECK (period_kind IN ('week', 'month')),
    period_start        date NOT NULL,
    project_id          uuid NOT NULL,
    load_minutes        integer NOT NULL CHECK (load_minutes > 0),
    provisional_minutes integer NOT NULL CHECK (provisional_minutes BETWEEN 0 AND load_minutes),
    capacity_minutes    integer NOT NULL CHECK (capacity_minutes >= 0),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, resource_id, period_kind, period_start, project_id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, project_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('load_aggregate_project');
-- What a node and a project read.
CREATE INDEX load_aggregate_project_idx ON load_aggregate_project (tenant_id, project_id, period_kind, period_start);

-- The shares of the assignments made before: one full recomputation per
-- tenant marks every resource's horizon, which the aggregates job stores.
SELECT app_queue_load_recompute_all();
