-- Invitations of new members, and groups of members (L02-US06).
-- Specification §3, §14.

-- Lists of members, in creation order (keyset pagination).
CREATE INDEX membership_tenant_created_idx ON membership (tenant_id, created_at, id);

-- ---------------------------------------------------------------------------
-- Invitations: a tenant admin invites an email with an account type and a
-- tenant role; the link of the email lets its owner join the tenant,
-- creating their global account or linking the existing one. Only the
-- SHA-256 of the token is stored. Expired invitations stay `pending` past
-- `expires_at` (the API reports them as expired) until sent again.
-- ---------------------------------------------------------------------------
CREATE TABLE invitation (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- Normalized by the application (trimmed, lower case).
    email        text NOT NULL
                 CHECK (email = lower(email) AND length(email) BETWEEN 3 AND 254
                        AND position('@' IN email) > 1),
    account_type text NOT NULL CHECK (account_type IN ('internal', 'external')),
    tenant_role  text NOT NULL DEFAULT 'member'
                 CHECK (tenant_role IN ('member', 'configuration_admin', 'tenant_admin')),
    token_hash   bytea NOT NULL,
    expires_at   timestamptz NOT NULL,
    status       text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'revoked')),
    -- The admin who sent it; NULL once their account is erased.
    invited_by   uuid REFERENCES account (id) ON DELETE SET NULL,
    accepted_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, token_hash),
    -- An external account never administers the tenant.
    CHECK (account_type = 'internal' OR tenant_role = 'member'),
    CHECK ((status = 'accepted') = (accepted_at IS NOT NULL))
);
-- One open invitation per email and tenant: sending again renews it.
CREATE UNIQUE INDEX invitation_tenant_pending_email_idx ON invitation (tenant_id, email)
    WHERE status = 'pending';
CREATE INDEX invitation_tenant_created_idx ON invitation (tenant_id, created_at, id);
SELECT app_setup_business_table('invitation');

-- ---------------------------------------------------------------------------
-- Groups of members: roles are granted to groups as well as to members
-- (L02-US07). A member's group memberships go with their membership.
-- ---------------------------------------------------------------------------
CREATE TABLE "group" (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100 AND name = btrim(name)),
    description text CHECK (length(description) <= 1000),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    -- Target of group_member's tenant-checked foreign key.
    UNIQUE (tenant_id, id)
);
-- A name is unique in its tenant, whatever its case.
CREATE UNIQUE INDEX group_tenant_name_idx ON "group" (tenant_id, lower(name));
CREATE INDEX group_tenant_created_idx ON "group" (tenant_id, created_at, id);
SELECT app_setup_business_table('"group"');

CREATE TABLE group_member (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    group_id   uuid NOT NULL,
    account_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, group_id, account_id),
    -- Both in the same tenant as the row: no group or member of another one.
    FOREIGN KEY (tenant_id, group_id) REFERENCES "group" (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, account_id) REFERENCES membership (tenant_id, account_id) ON DELETE CASCADE
);
CREATE INDEX group_member_tenant_account_idx ON group_member (tenant_id, account_id);
SELECT app_setup_business_table('group_member');
