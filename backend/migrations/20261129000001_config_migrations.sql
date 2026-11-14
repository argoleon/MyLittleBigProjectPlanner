-- Migrations of tickets when a configuration change is published
-- (L04-US08). Specification §5 and §17.
--
-- When the draft removes statuses or types that tickets use, the admin
-- maps each to one the draft keeps; this row records the mapping (by
-- `item_id`), and a background job (`config.migration`) moves the tickets
-- in batches, then publishes the draft — only once every ticket moved.
-- Tickets come with L05: until then there is none to move, and the job
-- publishes at once. One migration at a time per configuration.

CREATE TABLE config_migration (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    config_id        uuid NOT NULL,
    -- The draft it publishes; the job fails if the draft changed meanwhile.
    draft_id         uuid NOT NULL,
    state            text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'running', 'done', 'failed')),
    -- [{"from": item_id, "to": item_id}, …]
    status_mappings  jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(status_mappings) = 'array'),
    type_mappings    jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(type_mappings) = 'array'),
    ticket_total     bigint NOT NULL DEFAULT 0 CHECK (ticket_total >= 0),
    tickets_migrated bigint NOT NULL DEFAULT 0 CHECK (tickets_migrated >= 0),
    error            text,
    created_by       uuid,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz,
    FOREIGN KEY (tenant_id, config_id) REFERENCES project_config (tenant_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX config_migration_one_at_a_time ON config_migration (tenant_id, config_id)
    WHERE state IN ('pending', 'running');
CREATE INDEX config_migration_tenant_config_idx ON config_migration (tenant_id, config_id, created_at);
SELECT app_setup_business_table('config_migration');
