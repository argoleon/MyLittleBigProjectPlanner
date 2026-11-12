-- GDPR: export of personal data and erasure by anonymization (L02-US09).
-- Specification §14.
--
-- An erased member leaves their tenant, and the tenant shows them as
-- "Deleted user #N" from then on: `erased_member` keeps that pseudonym, so
-- that content they authored (tickets, comments, history: later work
-- packages) stays readable without naming them.
--
-- - A tenant admin erases a member of their tenant: the membership goes, the
--   tenant keeps the pseudonym; the global account and the other tenants are
--   untouched.
-- - The account itself, or a platform super admin, erases the global
--   account: every membership goes, every tenant keeps the same pseudonym,
--   and the account row is anonymized (name, email, password, preferences),
--   kept only for the references to it.
--
-- Each erasure queues a `gdpr.anonymize_member` job per tenant, where the
-- tables holding the member's content register their anonymization step
-- (worker, `infra::erasure`).

ALTER TABLE account
    ADD COLUMN erased_at timestamptz,
    ADD COLUMN pseudonym_number bigint UNIQUE;

-- Numbers of "Deleted user #N", never reused.
CREATE SEQUENCE pseudonym_number_seq;
GRANT USAGE ON SEQUENCE pseudonym_number_seq TO mlbpp_app;

CREATE TABLE erased_member (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- The account stays, anonymized or not: content refers to it.
    account_id       uuid NOT NULL REFERENCES account (id),
    pseudonym_number bigint NOT NULL,
    -- `tenant`: this tenant only; `account`: the global account.
    scope            text NOT NULL CHECK (scope IN ('tenant', 'account')),
    comments_purged  boolean NOT NULL,
    -- Who erased: the member, a tenant admin or a platform super admin.
    erased_by        uuid REFERENCES account (id) ON DELETE SET NULL,
    erased_at        timestamptz NOT NULL DEFAULT now(),
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    version          bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, account_id)
);
SELECT app_setup_business_table('erased_member');
CREATE POLICY erased_member_platform ON erased_member TO mlbpp_owner USING (true) WITH CHECK (true);

-- The erasure and the export below reach every tenant of an account: the
-- owner sees these tables whole, as it already sees memberships, MFA and
-- jobs.
CREATE POLICY invitation_platform ON invitation TO mlbpp_owner USING (true) WITH CHECK (true);
CREATE POLICY password_reset_platform ON password_reset TO mlbpp_owner USING (true) WITH CHECK (true);
CREATE POLICY group_platform ON "group" TO mlbpp_owner USING (true) WITH CHECK (true);
CREATE POLICY group_member_platform ON group_member TO mlbpp_owner USING (true) WITH CHECK (true);

-- Erases the global account `p_account` for `p_actor`, the signed-in
-- account (`app.account_id`): the account itself, or a platform super
-- admin. Returns the pseudonym number, or NULL when there is no such
-- account (or it was erased already). Raises `member.last_admin` when the
-- account is the last active tenant admin of a tenant, and
-- `account.platform_admin` when it is a platform super admin (another one
-- removes that role first).
CREATE FUNCTION app_erase_account(p_actor uuid, p_account uuid, p_purge_comments boolean)
    RETURNS bigint
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    v_email  text;
    v_number bigint;
BEGIN
    IF p_actor IS NULL OR p_actor IS DISTINCT FROM app_current_account() THEN
        RAISE EXCEPTION 'the actor must be the signed-in account' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF p_actor <> p_account THEN
        PERFORM app_require_platform_admin(p_actor);
    END IF;

    SELECT email INTO v_email FROM public.account WHERE id = p_account AND erased_at IS NULL FOR UPDATE;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM public.platform_admin WHERE account_id = p_account) THEN
        RAISE EXCEPTION 'account.platform_admin';
    END IF;

    -- The admin memberships of the account's tenants, locked in a fixed
    -- order: two admins erasing themselves at once cannot leave none.
    PERFORM 1
    FROM public.membership a
    WHERE a.tenant_role = 'tenant_admin' AND a.status = 'active' AND a.account_type = 'internal'
      AND a.tenant_id IN (SELECT m.tenant_id FROM public.membership m WHERE m.account_id = p_account)
    ORDER BY a.tenant_id, a.account_id
    FOR UPDATE;
    IF EXISTS (
        SELECT 1
        FROM public.membership m
        WHERE m.account_id = p_account
          AND m.tenant_role = 'tenant_admin' AND m.status = 'active' AND m.account_type = 'internal'
          AND NOT EXISTS (
              SELECT 1 FROM public.membership o
              WHERE o.tenant_id = m.tenant_id AND o.account_id <> p_account
                AND o.tenant_role = 'tenant_admin' AND o.status = 'active' AND o.account_type = 'internal'
          )
    ) THEN
        RAISE EXCEPTION 'member.last_admin';
    END IF;

    v_number := nextval('public.pseudonym_number_seq');

    -- Every tenant of the account keeps the pseudonym, a tenant that erased
    -- the member before included.
    INSERT INTO public.erased_member (id, tenant_id, account_id, pseudonym_number, scope, comments_purged, erased_by)
    SELECT uuidv7(), m.tenant_id, p_account, v_number, 'account', p_purge_comments, p_actor
    FROM public.membership m
    WHERE m.account_id = p_account
    ON CONFLICT (tenant_id, account_id) DO UPDATE
        SET pseudonym_number = EXCLUDED.pseudonym_number, scope = 'account',
            comments_purged = erased_member.comments_purged OR EXCLUDED.comments_purged,
            erased_by = EXCLUDED.erased_by, erased_at = now(), version = erased_member.version + 1;

    -- Each tenant anonymizes the member's content in its own context.
    INSERT INTO public.job (id, tenant_id, kind, payload)
    SELECT uuidv7(), e.tenant_id, 'gdpr.anonymize_member',
           jsonb_build_object('accountId', p_account, 'pseudonymNumber', v_number,
                              'purgeComments', p_purge_comments)
    FROM public.erased_member e
    WHERE e.account_id = p_account;

    -- Personal data held next to the account.
    DELETE FROM public.job j
    WHERE j.kind = 'email.send' AND j.status = 'queued' AND lower(j.payload ->> 'to') = v_email;
    DELETE FROM public.invitation WHERE email = v_email;
    DELETE FROM public.password_reset WHERE account_id = p_account;
    DELETE FROM public.account_backup_code WHERE account_id = p_account;
    DELETE FROM public.account_mfa WHERE account_id = p_account;
    -- Group memberships go with the memberships.
    DELETE FROM public.membership WHERE account_id = p_account;

    UPDATE public.account
    SET email = 'erased-' || v_number || '@erased.invalid',
        display_name = 'Deleted user #' || v_number,
        locale = NULL, time_zone = NULL, password_hash = NULL, password_changed_at = NULL,
        erased_at = now(), pseudonym_number = v_number, version = version + 1
    WHERE id = p_account;
    RETURN v_number;
END
$$;
REVOKE EXECUTE ON FUNCTION app_erase_account(uuid, uuid, boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_erase_account(uuid, uuid, boolean) TO mlbpp_app;

-- The personal data of the signed-in account (`app.account_id`), across
-- tenants, for its export: never another account's.
CREATE FUNCTION app_my_personal_data() RETURNS jsonb
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT jsonb_build_object(
        'account', jsonb_build_object(
            'id', a.id,
            'email', a.email,
            'displayName', a.display_name,
            'createdAt', a.created_at,
            'passwordChangedAt', a.password_changed_at,
            'hasPassword', a.password_hash IS NOT NULL
        ),
        'preferences', jsonb_build_object('locale', a.locale, 'timeZone', a.time_zone),
        'twoStepVerification', jsonb_build_object(
            'enabled', EXISTS (SELECT 1 FROM public.account_mfa f WHERE f.account_id = a.id AND f.enabled_at IS NOT NULL),
            'backupCodesLeft', (SELECT count(*) FROM public.account_backup_code c
                                WHERE c.account_id = a.id AND c.used_at IS NULL)
        ),
        'memberships', COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                'tenant', jsonb_build_object('id', t.id, 'slug', t.slug, 'name', t.name),
                'accountType', m.account_type,
                'tenantRole', m.tenant_role,
                'status', m.status,
                'localAccount', m.local_account,
                'memberSince', m.created_at,
                'groups', COALESCE((
                    SELECT jsonb_agg(g.name ORDER BY g.name)
                    FROM public.group_member gm
                    JOIN public."group" g ON g.tenant_id = gm.tenant_id AND g.id = gm.group_id
                    WHERE gm.tenant_id = m.tenant_id AND gm.account_id = a.id
                ), '[]'::jsonb)
            ) ORDER BY t.name, t.id)
            FROM public.membership m
            JOIN public.tenant t ON t.id = m.tenant_id
            WHERE m.account_id = a.id
        ), '[]'::jsonb)
    )
    FROM public.account a
    WHERE a.id = app_current_account() AND a.erased_at IS NULL
$$;
REVOKE EXECUTE ON FUNCTION app_my_personal_data() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_my_personal_data() TO mlbpp_app;

-- Purging a tenant keeps the accounts another tenant still shows as
-- "Deleted user #N" (the rest is unchanged).
CREATE OR REPLACE FUNCTION app_platform_purge_tenant(p_id uuid) RETURNS bigint
    LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tables text[];
    blocked text[];
    members uuid[];
    target text;
    removed bigint := 0;
    n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.tenant WHERE id = p_id AND status = 'deleting') THEN
        RAISE EXCEPTION 'tenant % is not being deleted', p_id USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
    -- Row-level security of the business tables applies to the owner too.
    PERFORM set_config('app.tenant_id', p_id::text, true);

    SELECT coalesce(array_agg(account_id), '{}') INTO members FROM public.membership WHERE tenant_id = p_id;

    SELECT coalesce(array_agg(c.relname::text ORDER BY c.relname), '{}') INTO tables
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace ns ON ns.oid = c.relnamespace
    JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
    WHERE ns.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relispartition = false
      AND a.attname = 'tenant_id' AND NOT a.attisdropped;

    WHILE cardinality(tables) > 0 LOOP
        blocked := '{}';
        FOREACH target IN ARRAY tables LOOP
            BEGIN
                EXECUTE format('DELETE FROM public.%I WHERE tenant_id = $1', target) USING p_id;
                GET DIAGNOSTICS n = ROW_COUNT;
                removed := removed + n;
            EXCEPTION WHEN foreign_key_violation THEN
                blocked := blocked || target;
            END;
        END LOOP;
        IF cardinality(blocked) = cardinality(tables) THEN
            RAISE EXCEPTION 'cannot purge tenant %: % reference each other', p_id, blocked;
        END IF;
        tables := blocked;
    END LOOP;

    DELETE FROM public.tenant WHERE id = p_id;
    removed := removed + 1;

    -- Accounts that only existed for this tenant (GDPR: all its data goes).
    DELETE FROM public.account a
    WHERE a.id = ANY (members)
      AND NOT EXISTS (SELECT 1 FROM public.membership m WHERE m.account_id = a.id)
      AND NOT EXISTS (SELECT 1 FROM public.platform_admin p WHERE p.account_id = a.id)
      -- Another tenant shows it as "Deleted user #N".
      AND NOT EXISTS (SELECT 1 FROM public.erased_member e WHERE e.account_id = a.id);
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN removed + n;
END
$$;
