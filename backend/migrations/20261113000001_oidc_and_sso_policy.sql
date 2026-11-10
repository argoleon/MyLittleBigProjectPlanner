-- Sign-in through the tenant's OIDC providers, and the rest of its
-- authentication policy (L02-US04). Specification §3, §14.
--
-- The key encrypting stored secrets is now DATA_ENCRYPTION_KEY (it was
-- MFA_ENCRYPTION_KEY, before any release): it also encrypts the OIDC client
-- secrets below.

ALTER TABLE tenant_auth_policy
    -- Internal members sign in through an OIDC provider of the tenant,
    -- except those marked as local accounts (membership.local_account).
    ADD COLUMN internal_accounts_require_sso boolean NOT NULL DEFAULT false,
    -- Sign-in methods of the request portal (enforced by the portal, L11).
    ADD COLUMN portal_password_enabled boolean NOT NULL DEFAULT true,
    ADD COLUMN portal_magic_link_enabled boolean NOT NULL DEFAULT true;

-- A member outside the organization's directory (a contractor): signs in
-- with a password even when the tenant requires SSO.
ALTER TABLE membership
    ADD COLUMN local_account boolean NOT NULL DEFAULT false;

-- The bootstrapped administrators keep a way in if the SSO breaks.
UPDATE membership m SET local_account = true
WHERE m.tenant_role = 'tenant_admin'
  AND EXISTS (SELECT 1 FROM platform_admin p WHERE p.account_id = m.account_id);

-- ---------------------------------------------------------------------------
-- OIDC providers of a tenant (Entra ID, Google, Keycloak…).
-- ---------------------------------------------------------------------------
CREATE TABLE oidc_provider (
    id                       uuid PRIMARY KEY,
    tenant_id                uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- Shown on the sign-in button: "Continue with <display_name>".
    display_name             text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 100),
    -- Issuer URL; its /.well-known/openid-configuration describes the rest.
    issuer                   text NOT NULL CHECK (issuer ~ '^https?://' AND length(issuer) <= 2000),
    client_id                text NOT NULL CHECK (length(client_id) BETWEEN 1 AND 500),
    -- AES-256-GCM (DATA_ENCRYPTION_KEY, the provider id as associated data).
    client_secret_ciphertext bytea NOT NULL,
    client_secret_nonce      bytea NOT NULL CHECK (length(client_secret_nonce) = 12),
    -- Claim holding the email matched against the members (verified only).
    email_claim              text NOT NULL DEFAULT 'email' CHECK (email_claim ~ '^[A-Za-z0-9_:.-]{1,100}$'),
    enabled                  boolean NOT NULL DEFAULT true,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now(),
    version                  bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, issuer, client_id)
);
CREATE INDEX oidc_provider_tenant_idx ON oidc_provider (tenant_id, created_at, id);
SELECT app_setup_business_table('oidc_provider');
