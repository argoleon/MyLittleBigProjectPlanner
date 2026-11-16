-- Links between tickets (L05-US03): blocks, depends on, duplicates, relates
-- to, across projects of the tenant. Stored from the source ticket to the
-- target; each reads one way from each side (`domain::links`).

CREATE TABLE issue_link (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    source_id   uuid NOT NULL,
    target_id   uuid NOT NULL,
    link_type   text NOT NULL CHECK (link_type IN ('blocks', 'depends_on', 'duplicates', 'relates_to')),
    created_by  uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT issue_link_not_self CHECK (source_id <> target_id),
    CONSTRAINT issue_link_unique UNIQUE (tenant_id, source_id, target_id, link_type),
    FOREIGN KEY (tenant_id, source_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, target_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);

-- "Relates to" reads the same both ways: once per pair, whatever its direction.
CREATE UNIQUE INDEX issue_link_relates_once ON issue_link
    (tenant_id, LEAST(source_id, target_id), GREATEST(source_id, target_id))
    WHERE link_type = 'relates_to';

-- The links of a ticket, from either side.
CREATE INDEX issue_link_tenant_target_idx ON issue_link (tenant_id, target_id);

SELECT app_setup_business_table('issue_link');
