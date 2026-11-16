-- Tickets (L05-US01) and the event outbox. Specification §5, §12, §17, §18.

-- The outbox: every change writes its event here in its own transaction
-- (`issue.created`…); one stream feeds real time, notifications and later
-- webhooks. Partitioned by month from the start (§18): a partition per
-- month, created ahead by `app_ensure_outbox_partitions` (the migration
-- and the worker call it), and a default partition as a safety net.
-- Events are purged once processed.
CREATE TABLE outbox_event (
    id             uuid NOT NULL,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    kind           text NOT NULL CHECK (kind ~ '^[a-z]+(\.[a-z_]+)+$'),
    aggregate_type text NOT NULL,
    aggregate_id   uuid NOT NULL,
    payload        jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(payload) = 'object'),
    -- Who made the change; none for the system.
    actor_id       uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    processed_at   timestamptz,
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);
SELECT app_setup_business_table('outbox_event');
CREATE INDEX outbox_event_pending_idx ON outbox_event (created_at) WHERE processed_at IS NULL;
CREATE INDEX outbox_event_tenant_aggregate_idx ON outbox_event (tenant_id, aggregate_type, aggregate_id);

-- A partition gets the rules of a business table: row-level security
-- forced, the tenant policy. The application reaches it through the parent.
CREATE FUNCTION app_secure_partition(target regclass) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
BEGIN
    EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', target);
    EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', target);
    IF NOT EXISTS (SELECT 1 FROM pg_policy WHERE polrelid = target AND polname = 'tenant_isolation') THEN
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %s '
            'USING (tenant_id = app_current_tenant()) '
            'WITH CHECK (tenant_id = app_current_tenant())',
            target);
    END IF;
END
$$;
REVOKE EXECUTE ON FUNCTION app_secure_partition(regclass) FROM PUBLIC;

CREATE TABLE outbox_event_default PARTITION OF outbox_event DEFAULT;
SELECT app_secure_partition('outbox_event_default');

-- The partitions of the current month and of the `months_ahead` next ones,
-- created if missing; the number created. The worker calls it every day.
CREATE FUNCTION app_ensure_outbox_partitions(months_ahead integer) RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    month   date := date_trunc('month', now() AT TIME ZONE 'UTC')::date;
    name    text;
    created integer := 0;
BEGIN
    FOR i IN 0..greatest(months_ahead, 0) LOOP
        name := format('outbox_event_%s', to_char(month + make_interval(months => i), 'YYYY_MM'));
        IF to_regclass('public.' || name) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE public.%I PARTITION OF public.outbox_event FOR VALUES FROM (%L) TO (%L)',
                name,
                (month + make_interval(months => i))::timestamp AT TIME ZONE 'UTC',
                (month + make_interval(months => i + 1))::timestamp AT TIME ZONE 'UTC');
            PERFORM public.app_secure_partition(('public.' || name)::regclass);
            created := created + 1;
        END IF;
    END LOOP;
    RETURN created;
END
$$;
REVOKE EXECUTE ON FUNCTION app_ensure_outbox_partitions(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_ensure_outbox_partitions(integer) TO mlbpp_app;
SELECT app_ensure_outbox_partitions(3);

-- The next key number of each project: a ticket takes it under the row
-- lock of the project's counter, so that two creations never share a key.
CREATE TABLE project_issue_counter (
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    project_id  uuid NOT NULL,
    last_number integer NOT NULL DEFAULT 0 CHECK (last_number >= 0),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, project_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('project_issue_counter');

-- A ticket. It belongs to one project; its type and status are those of
-- the project's configuration (`item_id`, stable from one version to the
-- next). `key` (`PLAT-12`) is unique in the tenant and kept as an alias if
-- the ticket moves (L05-US02). Custom field values sit in `fields`, by
-- field `item_id`, GIN-indexed for filtering on several at once.
CREATE TABLE issue (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    project_id  uuid NOT NULL,
    key_number  integer NOT NULL CHECK (key_number > 0),
    key         text NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$'),
    type_id     uuid NOT NULL,
    status_id   uuid NOT NULL,
    parent_id   uuid,
    summary     text NOT NULL CHECK (length(btrim(summary)) BETWEEN 1 AND 255),
    -- Rich text: a Tiptap document, rendered to sanitized HTML by the API.
    description jsonb CHECK (description IS NULL OR jsonb_typeof(description) = 'object'),
    priority    text NOT NULL DEFAULT 'medium' CHECK (priority IN ('highest', 'high', 'medium', 'low', 'lowest')),
    reporter_id uuid REFERENCES account (id) ON DELETE SET NULL,
    assignee_id uuid REFERENCES account (id) ON DELETE SET NULL,
    start_date  date,
    due_date    date CHECK (due_date IS NULL OR start_date IS NULL OR due_date >= start_date),
    fields      jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(fields) = 'object'),
    created_by  uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    CONSTRAINT issue_key_unique UNIQUE (tenant_id, key),
    UNIQUE (tenant_id, id),
    UNIQUE (project_id, key_number),
    FOREIGN KEY (tenant_id, project_id) REFERENCES node (tenant_id, id),
    FOREIGN KEY (tenant_id, parent_id) REFERENCES issue (tenant_id, id)
);
CREATE INDEX issue_tenant_project_idx ON issue (tenant_id, project_id, key_number);
CREATE INDEX issue_tenant_parent_idx ON issue (tenant_id, parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX issue_tenant_assignee_idx ON issue (tenant_id, assignee_id, updated_at) WHERE assignee_id IS NOT NULL;
CREATE INDEX issue_tenant_reporter_idx ON issue (tenant_id, reporter_id, updated_at) WHERE reporter_id IS NOT NULL;
CREATE INDEX issue_fields_idx ON issue USING gin (fields jsonb_path_ops);
SELECT app_setup_business_table('issue');

-- The former keys of a ticket that moved to another project (L05-US02):
-- `GET /issues/{key}` still finds it.
CREATE TABLE issue_key_alias (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    key        text NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$'),
    issue_id   uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT issue_key_alias_unique UNIQUE (tenant_id, key),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('issue_key_alias');

-- Identifiers of a ticket in another tool, for the Jira and OpenProject
-- imports (after the MVP): ready now, so that they need no migration.
CREATE TABLE external_ref (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    issue_id    uuid NOT NULL,
    system      text NOT NULL CHECK (system ~ '^[a-z][a-z0-9_]{0,39}$'),
    external_id text NOT NULL CHECK (length(external_id) BETWEEN 1 AND 255),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT external_ref_unique UNIQUE (tenant_id, system, external_id),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('external_ref');
