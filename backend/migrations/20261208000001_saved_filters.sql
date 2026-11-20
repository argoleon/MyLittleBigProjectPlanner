-- Saved filters (specification §6, L06-US04): a query of the language kept
-- under a name, personal or shared with groups or the whole tenant.
--
-- * The query is stored as written: it names stable identifiers (keys),
--   so it keeps working when labels change.
-- * Its owner alone edits it; a tenant admin may delete a shared one. The
--   owner is an account, not a membership: a shared filter outlives its
--   owner's departure (shown as a former member, or under their pseudonym
--   once erased). Their private filters go with their membership
--   (`infra::members::remove`, and the erasure job across tenants).
-- * Shared with the whole tenant (`shared_with_all`), or with groups
--   (`saved_filter_group`); private otherwise.

CREATE TABLE saved_filter (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- None: the account no longer exists.
    owner_id        uuid REFERENCES account (id) ON DELETE SET NULL,
    name            text NOT NULL CHECK (length(name) BETWEEN 1 AND 100 AND name = btrim(name)),
    icon            text NOT NULL,
    query           text NOT NULL CHECK (length(query) BETWEEN 1 AND 4000),
    shared_with_all boolean NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    version         bigint NOT NULL DEFAULT 1,
    -- Target of saved_filter_group's tenant-checked foreign key.
    UNIQUE (tenant_id, id)
);
-- A name is unique among its owner's filters, whatever its case.
CREATE UNIQUE INDEX saved_filter_owner_name_idx ON saved_filter (tenant_id, owner_id, lower(name));
CREATE INDEX saved_filter_tenant_shared_idx ON saved_filter (tenant_id) WHERE shared_with_all;
SELECT app_setup_business_table('saved_filter');

CREATE TABLE saved_filter_group (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    filter_id  uuid NOT NULL,
    group_id   uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, filter_id, group_id),
    -- Both in the same tenant as the row; a deleted group shares no more.
    FOREIGN KEY (tenant_id, filter_id) REFERENCES saved_filter (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, group_id) REFERENCES "group" (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX saved_filter_group_group_idx ON saved_filter_group (tenant_id, group_id);
SELECT app_setup_business_table('saved_filter_group');
