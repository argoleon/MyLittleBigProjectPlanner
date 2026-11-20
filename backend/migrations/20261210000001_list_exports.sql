-- Exports of list views (specification §6, L06-US06): the rows of a query
-- with the columns displayed, as a CSV file.
--
-- * Up to 1,000 rows, the API writes the file at once; beyond, the worker
--   does (`export.list` job) and the member is notified when it is ready.
-- * The file sits in the object storage for 24 hours, then the
--   `export.purge` job deletes it with its row. Only its requester reads it.
-- * The query, columns and language are kept as asked: the worker runs the
--   query with the requester's sight, as of when it runs.

CREATE TABLE list_export (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    account_id   uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- The file's name, as downloaded.
    file_name    text NOT NULL CHECK (length(file_name) BETWEEN 1 AND 200),
    format       text NOT NULL CHECK (format IN ('csv')),
    query        text NOT NULL CHECK (length(query) <= 10000),
    columns      text[] NOT NULL CHECK (cardinality(columns) BETWEEN 1 AND 40),
    -- The language of the headers and labels.
    language     text NOT NULL,
    status       text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'ready', 'failed')),
    row_count    integer CHECK (row_count >= 0),
    object_key   text,
    size_bytes   bigint CHECK (size_bytes >= 0),
    -- Why it failed, for its requester (a stable code).
    failure      text,
    completed_at timestamptz,
    expires_at   timestamptz NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, id),
    CONSTRAINT list_export_ready CHECK ((status = 'ready') = (object_key IS NOT NULL))
);
CREATE INDEX list_export_account_idx ON list_export (tenant_id, account_id, created_at DESC);
SELECT app_setup_business_table('list_export');

-- A notification may be about something else than a ticket: an export of
-- the member's ready (`export_ready`, no ticket; its export, file name and
-- row count in `details`). The constraints reach every partition.
ALTER TABLE notification ALTER COLUMN issue_id DROP NOT NULL;
ALTER TABLE notification DROP CONSTRAINT notification_event_check;
ALTER TABLE notification ADD CONSTRAINT notification_event_check
    CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned', 'export_ready'));
ALTER TABLE notification ADD CONSTRAINT notification_ticket
    CHECK ((event = 'export_ready') = (issue_id IS NULL));
