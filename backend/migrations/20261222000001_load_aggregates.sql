-- Weekly and monthly load of resources (specification §8, §18, L08-US06):
-- what the heatmap reads over long ranges instead of the daily rows.
--
-- * `load_aggregate`: per resource and week or month, the load (all of it,
--   and its provisional part), the capacity (absences deducted) and the
--   capacity scheduled before absences. Kept over a horizon of 12 months
--   back and 24 ahead (domain::workload::horizon).
-- * `load_aggregate_stale`: the periods to recompute. A change of load or
--   capacity marks them, and a `workload.aggregates` job, queued shortly
--   after, recomputes them all (infra::load_aggregates). A period the
--   heatmap finds missing is computed on the fly and marked too.

CREATE TABLE load_aggregate (
    tenant_id           uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id         uuid NOT NULL,
    period_kind         text NOT NULL CHECK (period_kind IN ('week', 'month')),
    -- A Monday, or the first day of a month.
    period_start        date NOT NULL,
    load_minutes        integer NOT NULL CHECK (load_minutes >= 0),
    provisional_minutes integer NOT NULL CHECK (provisional_minutes BETWEEN 0 AND load_minutes),
    capacity_minutes    integer NOT NULL CHECK (capacity_minutes >= 0),
    scheduled_minutes   integer NOT NULL CHECK (scheduled_minutes >= capacity_minutes),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, resource_id, period_kind, period_start),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('load_aggregate');

CREATE TABLE load_aggregate_stale (
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id  uuid NOT NULL,
    period_kind  text NOT NULL CHECK (period_kind IN ('week', 'month')),
    period_start date NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, resource_id, period_kind, period_start),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('load_aggregate_stale');
