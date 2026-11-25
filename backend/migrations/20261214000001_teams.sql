-- Teams (specification §8, L07-US02): resources grouped for views and
-- filters. A resource may belong to several teams; assignments always
-- target a resource, never a team (L08).
--
-- * The name is translatable (`{"fr": …, "en": …}`), unique in the tenant
--   per language whatever its case.
-- * Deleting a team deletes its memberships only; deleting a resource
--   (never done by the application: resources are deactivated) would too.

CREATE TABLE team (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    name        jsonb NOT NULL CHECK (jsonb_typeof(name) = 'object' AND name <> '{}'::jsonb),
    description text CHECK (description IS NULL OR (length(description) BETWEEN 1 AND 1000)),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    -- Target of team_member's tenant-checked foreign key.
    UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX team_tenant_name_fr_idx ON team (tenant_id, lower(name ->> 'fr'));
CREATE UNIQUE INDEX team_tenant_name_en_idx ON team (tenant_id, lower(name ->> 'en'));
SELECT app_setup_business_table('team');

CREATE TABLE team_member (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    team_id     uuid NOT NULL,
    resource_id uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, team_id, resource_id),
    -- Both in the same tenant as the row.
    FOREIGN KEY (tenant_id, team_id) REFERENCES team (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX team_member_resource_idx ON team_member (tenant_id, resource_id);
SELECT app_setup_business_table('team_member');
