-- Accounts, memberships and password resets (L02-US02), and the job queue
-- of the worker (L02-US01-T03). Specification §3, §14, §17.

-- ---------------------------------------------------------------------------
-- Accounts: global, a person has one account and can belong to several
-- tenants. The only business table without tenant_id (migrations/README.md).
-- ---------------------------------------------------------------------------
CREATE TABLE account (
    id                  uuid PRIMARY KEY,
    -- Normalized by the application (trimmed, lower case).
    email               text NOT NULL UNIQUE
                        CHECK (email = lower(email) AND length(email) BETWEEN 3 AND 254
                               AND position('@' IN email) > 1),
    display_name        text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 200),
    -- Preferred language; the tenant's default when NULL.
    locale              text CHECK (locale IN ('fr', 'en')),
    -- Argon2id hash (PHC string); NULL when the account has no local
    -- password (SSO only, invitation not accepted yet).
    password_hash       text,
    password_changed_at timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    version             bigint NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- Memberships: an account in a tenant, with its account type and tenant role.
-- ---------------------------------------------------------------------------
CREATE TABLE membership (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    account_id   uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- internal: the application; external: the request portal only (§3).
    account_type text NOT NULL CHECK (account_type IN ('internal', 'external')),
    tenant_role  text NOT NULL DEFAULT 'member'
                 CHECK (tenant_role IN ('member', 'configuration_admin', 'tenant_admin')),
    status       text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, account_id),
    -- An external account never administers the tenant.
    CHECK (account_type = 'internal' OR tenant_role = 'member')
);
SELECT app_setup_business_table('membership');

-- Row-level security of accounts: the services see and update the accounts
-- of the current tenant's members only (membership itself is tenant
-- scoped), so an email unknown to the tenant looks like an unknown email.
-- They may create accounts (invitations, sign-up of a member); finding an
-- account across tenants goes through app_account_id_by_email.
-- Platform functions (SECURITY DEFINER, owned by mlbpp_owner) see them all.
ALTER TABLE account ENABLE ROW LEVEL SECURITY;
ALTER TABLE account FORCE ROW LEVEL SECURITY;
CREATE POLICY account_of_member ON account FOR SELECT TO mlbpp_app
    USING (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account.id));
CREATE POLICY account_update_of_member ON account FOR UPDATE TO mlbpp_app
    USING (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account.id))
    WITH CHECK (EXISTS (SELECT 1 FROM membership m WHERE m.account_id = account.id));
CREATE POLICY account_create ON account FOR INSERT TO mlbpp_app WITH CHECK (true);
CREATE POLICY account_platform ON account TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, UPDATE ON account TO mlbpp_app;
CREATE TRIGGER touch_updated_at BEFORE UPDATE ON account
    FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at();

-- Id of the account holding `p_email` (normalized), whatever its tenants:
-- invitations and the `api add-member` command link an existing account.
CREATE FUNCTION app_account_id_by_email(p_email text) RETURNS uuid
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT id FROM public.account WHERE email = p_email
$$;
REVOKE EXECUTE ON FUNCTION app_account_id_by_email(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_account_id_by_email(text) TO mlbpp_app;

-- ---------------------------------------------------------------------------
-- Password resets: requested on a tenant's address, so tenant scoped. Only
-- the SHA-256 of the token is stored; a reset is single use.
-- ---------------------------------------------------------------------------
CREATE TABLE password_reset (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    account_id uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, token_hash)
);
CREATE INDEX password_reset_tenant_account_idx ON password_reset (tenant_id, account_id);
SELECT app_setup_business_table('password_reset');

-- ---------------------------------------------------------------------------
-- Job queue (specification §17): jobs are enqueued in a tenant transaction,
-- and claimed by the workers across tenants with FOR UPDATE SKIP LOCKED, so
-- each job runs on one worker only. A claimed job holds a lease: if its
-- worker dies, the job is claimed again once the lease expires.
-- Completed jobs are deleted; failed ones stay for inspection.
-- ---------------------------------------------------------------------------
CREATE TABLE job (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    kind         text NOT NULL CHECK (kind ~ '^[a-z][a-z0-9_.]*$'),
    payload      jsonb NOT NULL DEFAULT '{}',
    status       text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'failed')),
    attempts     integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts > 0),
    run_at       timestamptz NOT NULL DEFAULT now(),
    locked_by    text,
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1
);
-- The one index not starting with tenant_id: the workers look for due jobs
-- of every tenant.
CREATE INDEX job_due_idx ON job (run_at) WHERE status IN ('queued', 'running');
SELECT app_setup_business_table('job');
-- The queue functions below, owned by mlbpp_owner, work across tenants.
CREATE POLICY job_platform ON job TO mlbpp_owner USING (true) WITH CHECK (true);

-- Claims up to `p_limit` due jobs for `p_worker`, for `p_lease_secs` seconds.
-- A job whose lease expired after its last attempt is marked failed first.
CREATE FUNCTION app_claim_jobs(p_worker text, p_limit integer, p_lease_secs integer)
    RETURNS TABLE (id uuid, tenant_id uuid, kind text, payload jsonb, attempts integer, max_attempts integer)
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.job AS j
    SET status = 'failed', locked_by = NULL, locked_until = NULL,
        last_error = coalesce(j.last_error, '') || ' [lease expired after the last attempt]'
    WHERE j.status = 'running' AND j.locked_until < now() AND j.attempts >= j.max_attempts;

    RETURN QUERY
    UPDATE public.job AS j
    SET status = 'running',
        attempts = j.attempts + 1,
        locked_by = p_worker,
        locked_until = now() + make_interval(secs => p_lease_secs)
    WHERE j.id IN (
        SELECT q.id FROM public.job AS q
        WHERE (q.status = 'queued' AND q.run_at <= now())
           OR (q.status = 'running' AND q.locked_until < now())
        ORDER BY q.run_at
        FOR UPDATE SKIP LOCKED
        LIMIT p_limit
    )
    RETURNING j.id, j.tenant_id, j.kind, j.payload, j.attempts, j.max_attempts;
END
$$;

-- A job done by `p_worker`: deleted. False if the worker lost its lease.
CREATE FUNCTION app_complete_job(p_id uuid, p_worker text) RETURNS boolean
    LANGUAGE sql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    WITH done AS (
        DELETE FROM public.job
        WHERE id = p_id AND locked_by = p_worker AND status = 'running'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM done)
$$;

-- A job `p_worker` could not do: queued again at `p_retry_at`, or failed for
-- good when `p_retry_at` is NULL. False if the worker lost its lease.
CREATE FUNCTION app_fail_job(p_id uuid, p_worker text, p_error text, p_retry_at timestamptz)
    RETURNS boolean
    LANGUAGE sql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    WITH changed AS (
        UPDATE public.job
        SET status = CASE WHEN p_retry_at IS NULL THEN 'failed' ELSE 'queued' END,
            run_at = coalesce(p_retry_at, run_at),
            locked_by = NULL,
            locked_until = NULL,
            last_error = left(p_error, 2000)
        WHERE id = p_id AND locked_by = p_worker AND status = 'running'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM changed)
$$;

REVOKE EXECUTE ON FUNCTION app_claim_jobs(text, integer, integer) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_complete_job(uuid, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION app_fail_job(uuid, text, text, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_claim_jobs(text, integer, integer) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_complete_job(uuid, text) TO mlbpp_app;
GRANT EXECUTE ON FUNCTION app_fail_job(uuid, text, text, timestamptz) TO mlbpp_app;
