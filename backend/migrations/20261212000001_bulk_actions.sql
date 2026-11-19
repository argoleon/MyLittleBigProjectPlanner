-- Bulk actions on a search result (L06-US08): several tickets modified,
-- moved through their workflow, or moved to another project at once.
--
-- * Each ticket is changed in its own transaction: its item records what
--   became of it (changed, unchanged, refused with the reason, or moved
--   with its parent), in that same transaction.
-- * Up to 100 tickets, the API changes them within the request; beyond,
--   the worker does (`bulk.item`, one job per ticket), counting down
--   `remaining`, and the member is notified when none is left.
-- * Kept 7 days (`bulk.purge`), for its requester only.

CREATE TABLE bulk_action (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    account_id   uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    kind         text NOT NULL CHECK (kind IN ('modify', 'transition', 'move')),
    -- What to do, as asked (`api::bulk_actions::ActionBody`).
    params       jsonb NOT NULL CHECK (jsonb_typeof(params) = 'object'),
    -- What the report is written in.
    language     text NOT NULL,
    background   boolean NOT NULL,
    total        integer NOT NULL CHECK (total BETWEEN 1 AND 10000),
    remaining    integer NOT NULL CHECK (remaining >= 0 AND remaining <= total),
    completed_at timestamptz,
    expires_at   timestamptz NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, id)
);
CREATE INDEX bulk_action_account_idx ON bulk_action (tenant_id, account_id, created_at DESC);
SELECT app_setup_business_table('bulk_action');

CREATE TABLE bulk_action_item (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    bulk_id    uuid NOT NULL,
    position   integer NOT NULL CHECK (position >= 0),
    -- The ticket; no foreign key: the report outlives a ticket deleted since.
    issue_id   uuid NOT NULL,
    -- Its key when selected.
    issue_key  text NOT NULL,
    -- Null while waiting.
    outcome    text CHECK (outcome IN ('changed', 'unchanged', 'refused', 'with_parent')),
    -- A stable code, when refused (`domain::bulk::reason`).
    reason     text,
    -- The field errors, when refused for its values: [{field, code}].
    errors     jsonb,
    -- Its key once moved.
    new_key    text,
    done_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, bulk_id, position),
    FOREIGN KEY (tenant_id, bulk_id) REFERENCES bulk_action (tenant_id, id) ON DELETE CASCADE,
    CONSTRAINT bulk_action_item_reason CHECK ((outcome = 'refused') = (reason IS NOT NULL))
);
SELECT app_setup_business_table('bulk_action_item');

-- The member is told when a background bulk action is done: a notice of
-- their own (`bulk_done`, no ticket; the action and its counts in
-- `details`).
ALTER TABLE notification DROP CONSTRAINT notification_event_check;
ALTER TABLE notification ADD CONSTRAINT notification_event_check
    CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned', 'export_ready',
                     'bulk_done'));
ALTER TABLE notification DROP CONSTRAINT notification_ticket;
ALTER TABLE notification ADD CONSTRAINT notification_ticket
    CHECK ((event IN ('export_ready', 'bulk_done')) = (issue_id IS NULL));
