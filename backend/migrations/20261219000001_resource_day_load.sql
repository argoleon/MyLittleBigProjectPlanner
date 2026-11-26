-- Daily load of resources (specification §8, §18, L08-US02): one row per
-- assignment and day it loads, derived from the segments and the
-- resource's capacity, calendars and absences. Recomputed in the same
-- transaction when an assignment is written, and by the worker when a
-- capacity, a calendar or an absence changes. Read by the overload check,
-- the heatmap and the availability search.
--
-- Partitioned by month of the day (§18): a partition per month, created
-- by `app_ensure_load_partitions` before rows are written there (the worker
-- also creates the coming months ahead), and a default partition as a
-- safety net.
CREATE TABLE resource_day_load (
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id   uuid NOT NULL,
    day           date NOT NULL,
    assignment_id uuid NOT NULL,
    issue_id      uuid NOT NULL,
    -- Never 0: a day the assignment does not load has no row. May exceed a
    -- day (40 h of effort over one working day): the overload check says so.
    minutes       integer NOT NULL CHECK (minutes > 0),
    provisional   boolean NOT NULL,
    PRIMARY KEY (tenant_id, assignment_id, day),
    -- Its rows go with the assignment (and the assignment with its ticket).
    FOREIGN KEY (tenant_id, assignment_id) REFERENCES assignment (tenant_id, id) ON DELETE CASCADE
) PARTITION BY RANGE (day);
-- What the overload check, the heatmap and the availability search read.
CREATE INDEX resource_day_load_resource_idx ON resource_day_load (tenant_id, resource_id, day);
ALTER TABLE resource_day_load ENABLE ROW LEVEL SECURITY;
ALTER TABLE resource_day_load FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON resource_day_load
    USING (tenant_id = app_current_tenant())
    WITH CHECK (tenant_id = app_current_tenant());
GRANT SELECT, INSERT, UPDATE, DELETE ON resource_day_load TO mlbpp_app;

CREATE TABLE resource_day_load_default PARTITION OF resource_day_load DEFAULT;
SELECT app_secure_partition('resource_day_load_default');

-- The partitions of every month from `first` to `last` (local dates),
-- created if missing; the number created. Called before load rows are
-- written, and by the worker for the coming months. Creations are
-- serialized: two transactions never create the same partition.
CREATE FUNCTION app_ensure_load_partitions(first date, last date) RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    month   date;
    name    text;
    created integer := 0;
    locked  boolean := false;
BEGIN
    IF first IS NULL OR last IS NULL OR last < first THEN
        RETURN 0;
    END IF;
    month := date_trunc('month', first)::date;
    WHILE month <= last LOOP
        name := format('resource_day_load_%s', to_char(month, 'YYYY_MM'));
        IF to_regclass('public.' || name) IS NULL THEN
            IF NOT locked THEN
                PERFORM pg_advisory_xact_lock(hashtext('public.resource_day_load partitions'));
                locked := true;
            END IF;
            -- Checked again under the lock: another transaction may have made it.
            IF to_regclass('public.' || name) IS NULL THEN
                EXECUTE format(
                    'CREATE TABLE public.%I PARTITION OF public.resource_day_load FOR VALUES FROM (%L) TO (%L)',
                    name, month, (month + interval '1 month')::date);
                PERFORM public.app_secure_partition(('public.' || name)::regclass);
                created := created + 1;
            END IF;
        END IF;
        month := (month + interval '1 month')::date;
    END LOOP;
    RETURN created;
END
$$;
REVOKE EXECUTE ON FUNCTION app_ensure_load_partitions(date, date) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_ensure_load_partitions(date, date) TO mlbpp_app;
-- The year before and the two years after the current month.
SELECT app_ensure_load_partitions(
    (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '12 months')::date,
    (date_trunc('month', now() AT TIME ZONE 'UTC') + interval '24 months')::date
);

-- Queues one full recomputation of the daily load per tenant with
-- assignments (job `workload.recompute`, run by the worker: `infra::day_load`);
-- the number queued. For assignments made before the daily load existed.
CREATE FUNCTION app_queue_load_recompute_all() RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    t uuid;
    queued integer := 0;
BEGIN
    FOR t IN SELECT id FROM public.tenant LOOP
        -- Row-level security applies to the owner too: work in each tenant.
        PERFORM set_config('app.tenant_id', t::text, true);
        IF EXISTS (SELECT 1 FROM public.assignment) THEN
            INSERT INTO public.job (id, tenant_id, kind, payload)
            VALUES (gen_random_uuid(), t, 'workload.recompute', '{"scope": {"kind": "all"}, "from": null, "until": null}');
            queued := queued + 1;
        END IF;
    END LOOP;
    PERFORM set_config('app.tenant_id', '', true);
    RETURN queued;
END
$$;
REVOKE EXECUTE ON FUNCTION app_queue_load_recompute_all() FROM PUBLIC;
-- Assignments made before this migration have no load yet.
SELECT app_queue_load_recompute_all();
