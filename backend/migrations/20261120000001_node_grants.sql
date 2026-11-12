-- Grants of roles on hierarchy nodes (L03-US03). Specification §4, §14.
--
-- A role is granted to a member or to a group, on a node. It applies to the
-- node and to its descendants, except through a confidential node, which
-- only its own grants reach. With several parents, access is the union of
-- what each one brings: a grant only adds permissions.

-- Target of the tenant-checked foreign key below.
ALTER TABLE role ADD CONSTRAINT role_tenant_id_id_key UNIQUE (tenant_id, id);

CREATE TABLE node_role_grant (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    node_id    uuid NOT NULL,
    role_id    uuid NOT NULL,
    -- Who receives the role: a member, or a group.
    account_id uuid,
    group_id   uuid,
    created_by uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    CONSTRAINT node_role_grant_one_principal CHECK ((account_id IS NULL) <> (group_id IS NULL)),
    -- All in the same tenant as the row. Grants go with their node, their
    -- member (GDPR erasure included) or their group.
    CONSTRAINT node_role_grant_node_fkey
        FOREIGN KEY (tenant_id, node_id) REFERENCES node (tenant_id, id) ON DELETE CASCADE,
    -- A role in use is not deleted: the API refuses first (`role.in_use`).
    -- Checked at the end of the statement, so that a tenant's deletion can
    -- cascade to both tables.
    CONSTRAINT node_role_grant_role_fkey
        FOREIGN KEY (tenant_id, role_id) REFERENCES role (tenant_id, id),
    CONSTRAINT node_role_grant_member_fkey
        FOREIGN KEY (tenant_id, account_id) REFERENCES membership (tenant_id, account_id) ON DELETE CASCADE,
    CONSTRAINT node_role_grant_group_fkey
        FOREIGN KEY (tenant_id, group_id) REFERENCES "group" (tenant_id, id) ON DELETE CASCADE
);
-- A role is granted once to a member or a group on a node.
CREATE UNIQUE INDEX node_role_grant_member_idx
    ON node_role_grant (tenant_id, node_id, role_id, account_id) WHERE account_id IS NOT NULL;
CREATE UNIQUE INDEX node_role_grant_group_idx
    ON node_role_grant (tenant_id, node_id, role_id, group_id) WHERE group_id IS NOT NULL;
-- What reaches a member: their grants, and those of their groups.
CREATE INDEX node_role_grant_tenant_account_idx
    ON node_role_grant (tenant_id, account_id) WHERE account_id IS NOT NULL;
CREATE INDEX node_role_grant_tenant_group_idx
    ON node_role_grant (tenant_id, group_id) WHERE group_id IS NOT NULL;
CREATE INDEX node_role_grant_tenant_role_idx ON node_role_grant (tenant_id, role_id);
SELECT app_setup_business_table('node_role_grant');
CREATE POLICY node_role_grant_platform ON node_role_grant TO mlbpp_owner USING (true) WITH CHECK (true);
