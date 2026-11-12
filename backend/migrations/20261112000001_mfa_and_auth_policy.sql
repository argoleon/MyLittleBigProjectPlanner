-- Two-step verification (TOTP and backup codes) and the tenant's
-- authentication policy (L02-US03). Specification §3, §14.

-- ---------------------------------------------------------------------------
-- Authentication policy of a tenant, set by its tenant admin. One row per
-- tenant, created with it. L02-US04 adds SSO and portal settings here.
-- ---------------------------------------------------------------------------
CREATE TABLE tenant_auth_policy (
    id                         uuid PRIMARY KEY,
    tenant_id                  uuid NOT NULL UNIQUE REFERENCES tenant (id) ON DELETE CASCADE,
    -- Members signing in with a password must use two-step verification.
    local_accounts_require_mfa boolean NOT NULL DEFAULT false,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    version                    bigint NOT NULL DEFAULT 1
);
SELECT app_setup_business_table('tenant_auth_policy');
-- Created by the trigger below, whoever creates the tenant.
CREATE POLICY tenant_auth_policy_platform ON tenant_auth_policy TO mlbpp_owner USING (true) WITH CHECK (true);

CREATE FUNCTION app_create_tenant_defaults() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    -- A UUID v7 like every key: 48-bit milliseconds, version 7, variant 2.
    INSERT INTO public.tenant_auth_policy (id, tenant_id) VALUES (uuidv7(), NEW.id);
    RETURN NEW;
END
$$;
CREATE TRIGGER create_defaults AFTER INSERT ON tenant
    FOR EACH ROW EXECUTE FUNCTION app_create_tenant_defaults();

INSERT INTO tenant_auth_policy (id, tenant_id) SELECT uuidv7(), id FROM tenant;

-- ---------------------------------------------------------------------------
-- Two-step verification of an account: global, like the account (one
-- authenticator for every tenant). The services see the rows of the current
-- tenant's members only, like accounts (migrations/README.md).
-- ---------------------------------------------------------------------------
CREATE TABLE account_mfa (
    account_id        uuid PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    -- TOTP secret, encrypted with AES-256-GCM (key MFA_ENCRYPTION_KEY,
    -- the account id as associated data).
    secret_ciphertext bytea NOT NULL,
    secret_nonce      bytea NOT NULL CHECK (length(secret_nonce) = 12),
    -- NULL while enrollment waits for its first code.
    enabled_at        timestamptz,
    -- Time step of the last accepted code: a code is never accepted twice.
    last_used_step    bigint,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    version           bigint NOT NULL DEFAULT 1
);

-- Backup codes: single use, stored as SHA-256 (they are random, 50 bits).
CREATE TABLE account_backup_code (
    id         uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    code_hash  bytea NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    UNIQUE (account_id, code_hash)
);

ALTER TABLE account_mfa ENABLE ROW LEVEL SECURITY;
ALTER TABLE account_mfa FORCE ROW LEVEL SECURITY;
CREATE POLICY account_mfa_of_member ON account_mfa TO mlbpp_app
    USING (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account_mfa.account_id))
    WITH CHECK (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account_mfa.account_id));
CREATE POLICY account_mfa_platform ON account_mfa TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, UPDATE, DELETE ON account_mfa TO mlbpp_app;
CREATE TRIGGER touch_updated_at BEFORE UPDATE ON account_mfa
    FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at();

ALTER TABLE account_backup_code ENABLE ROW LEVEL SECURITY;
ALTER TABLE account_backup_code FORCE ROW LEVEL SECURITY;
CREATE POLICY account_backup_code_of_member ON account_backup_code TO mlbpp_app
    USING (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account_backup_code.account_id))
    WITH CHECK (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account_backup_code.account_id));
CREATE POLICY account_backup_code_platform ON account_backup_code TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, UPDATE, DELETE ON account_backup_code TO mlbpp_app;
CREATE TRIGGER touch_updated_at BEFORE UPDATE ON account_backup_code
    FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at();
