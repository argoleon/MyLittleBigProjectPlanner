-- Notifications (L05-US07). Specification §12, §18.
--
-- Watchers of tickets; notifications, written by the worker from the
-- outbox; each account's preferences, which apply in every organization.
-- A member reads their notifications of all their organizations at once,
-- through the functions below only: each answers for the signed-in account
-- (`app.account_id`) and its active memberships, nothing else.

-- Those who follow a ticket: its reporter, its assignee and those who
-- comment on it start watching it; anyone may watch or stop.
CREATE TABLE issue_watcher (
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    issue_id   uuid NOT NULL,
    account_id uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, issue_id, account_id),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('issue_watcher');

-- A notification of a member about a ticket. Written once per outbox event
-- and recipient (`outbox_event_id`); `in_app` when the member sees it in
-- the application, `email_wanted` when it goes by email, `emailed_at` once
-- it went (alone, or in the digest). Partitioned by month (§18), like the
-- outbox.
CREATE TABLE notification (
    id              uuid NOT NULL,
    tenant_id       uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    account_id      uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    event           text NOT NULL
                    CHECK (event IN ('created', 'assigned', 'status_changed', 'commented', 'mentioned')),
    issue_id        uuid NOT NULL,
    -- Who made the change.
    actor_id        uuid REFERENCES account (id) ON DELETE SET NULL,
    -- What to tell, as of the change: key, summary, statuses, excerpt…
    details         jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(details) = 'object'),
    outbox_event_id uuid NOT NULL,
    in_app          boolean NOT NULL,
    email_wanted    boolean NOT NULL,
    emailed_at      timestamptz,
    read_at         timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (id, created_at),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
) PARTITION BY RANGE (created_at);
SELECT app_setup_business_table('notification');
-- The functions below (owned by mlbpp_owner) read a member's notifications
-- across tenants; the application role keeps the tenant policy.
CREATE POLICY notification_platform ON notification TO mlbpp_owner USING (true) WITH CHECK (true);
CREATE INDEX notification_account_idx ON notification (account_id, created_at DESC, id DESC);
CREATE INDEX notification_digest_idx ON notification (account_id) WHERE email_wanted AND emailed_at IS NULL;
CREATE INDEX notification_event_idx ON notification (tenant_id, outbox_event_id);

CREATE TABLE notification_default PARTITION OF notification DEFAULT;
SELECT app_secure_partition('notification_default');

-- The partitions of the current month and of the `months_ahead` next ones,
-- created if missing; the number created. The worker calls it every day.
CREATE FUNCTION app_ensure_notification_partitions(months_ahead integer) RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    month   date := date_trunc('month', now() AT TIME ZONE 'UTC')::date;
    name    text;
    created integer := 0;
BEGIN
    FOR i IN 0..greatest(months_ahead, 0) LOOP
        name := format('notification_%s', to_char(month + make_interval(months => i), 'YYYY_MM'));
        IF to_regclass('public.' || name) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE public.%I PARTITION OF public.notification FOR VALUES FROM (%L) TO (%L)',
                name,
                (month + make_interval(months => i))::timestamp AT TIME ZONE 'UTC',
                (month + make_interval(months => i + 1))::timestamp AT TIME ZONE 'UTC');
            PERFORM public.app_secure_partition(('public.' || name)::regclass);
            created := created + 1;
        END IF;
    END LOOP;
    RETURN created;
END
$$;
REVOKE EXECUTE ON FUNCTION app_ensure_notification_partitions(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_ensure_notification_partitions(integer) TO mlbpp_app;
SELECT app_ensure_notification_partitions(3);

-- An account's preferences, for every organization: per event, in the
-- application and by email (absent: the defaults of `domain::notify`), and
-- the daily digest. Not tenant data: row-level security keys it on the
-- signed-in account.
CREATE TABLE notification_settings (
    account_id     uuid PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    -- {"mentioned": {"inApp": true, "email": true}, …}
    preferences    jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(preferences) = 'object'),
    daily_digest   boolean NOT NULL DEFAULT false,
    -- The member's local date of the last digest sent.
    last_digest_on date,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE notification_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY notification_settings_own ON notification_settings TO mlbpp_app
    USING (account_id = app_current_account()) WITH CHECK (account_id = app_current_account());
CREATE POLICY notification_settings_platform ON notification_settings TO mlbpp_owner USING (true) WITH CHECK (true);
GRANT SELECT, INSERT, UPDATE ON notification_settings TO mlbpp_app;
CREATE TRIGGER touch_updated_at BEFORE UPDATE ON notification_settings
    FOR EACH ROW EXECUTE FUNCTION app_touch_updated_at();

-- The tenants the signed-in account reads notifications of: its active
-- memberships of active tenants.
CREATE FUNCTION app_my_notification_tenants() RETURNS SETOF uuid
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT m.tenant_id FROM public.membership m JOIN public.tenant t ON t.id = m.tenant_id
    WHERE m.account_id = app_current_account() AND m.status = 'active' AND t.status = 'active'
$$;
REVOKE EXECUTE ON FUNCTION app_my_notification_tenants() FROM PUBLIC;

-- The name `p_account` shows under in `p_tenant`: "Deleted user #N" once
-- erased there.
CREATE FUNCTION app_member_name_in(p_tenant uuid, p_account uuid) RETURNS text
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT coalesce(
        (SELECT 'Deleted user #' || pseudonym_number FROM public.erased_member
         WHERE tenant_id = p_tenant AND account_id = p_account),
        (SELECT display_name FROM public.account WHERE id = p_account))
$$;
REVOKE EXECUTE ON FUNCTION app_member_name_in(uuid, uuid) FROM PUBLIC;

-- The signed-in account's notifications shown in the application, of all
-- its organizations (or `p_tenant`), newest first, before the position
-- (`p_before_at`, `p_before_id`) if given.
CREATE FUNCTION app_my_notifications(
    p_tenant uuid, p_unread_only boolean, p_before_at timestamptz, p_before_id uuid, p_limit integer
) RETURNS TABLE (
    id uuid, tenant_id uuid, tenant_slug text, tenant_name text, event text, issue_id uuid,
    actor_id uuid, actor_name text, details jsonb, read_at timestamptz, created_at timestamptz
)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT n.id, n.tenant_id, t.slug, t.name, n.event, n.issue_id, n.actor_id,
           public.app_member_name_in(n.tenant_id, n.actor_id), n.details, n.read_at, n.created_at
    FROM public.notification n
    JOIN public.tenant t ON t.id = n.tenant_id
    WHERE n.account_id = app_current_account()
      AND n.in_app
      AND n.tenant_id IN (SELECT public.app_my_notification_tenants())
      AND (p_tenant IS NULL OR n.tenant_id = p_tenant)
      AND (NOT p_unread_only OR n.read_at IS NULL)
      AND (p_before_at IS NULL OR (n.created_at, n.id) < (p_before_at, p_before_id))
    ORDER BY n.created_at DESC, n.id DESC
    LIMIT greatest(p_limit, 0)
$$;
REVOKE EXECUTE ON FUNCTION app_my_notifications(uuid, boolean, timestamptz, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_my_notifications(uuid, boolean, timestamptz, uuid, integer) TO mlbpp_app;

-- How many notifications the signed-in account has not read, per
-- organization.
CREATE FUNCTION app_my_unread_notifications() RETURNS TABLE (tenant_id uuid, unread bigint)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT n.tenant_id, count(*)
    FROM public.notification n
    WHERE n.account_id = app_current_account() AND n.in_app AND n.read_at IS NULL
      AND n.tenant_id IN (SELECT public.app_my_notification_tenants())
    GROUP BY n.tenant_id
$$;
REVOKE EXECUTE ON FUNCTION app_my_unread_notifications() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_my_unread_notifications() TO mlbpp_app;

-- Marks read the signed-in account's notifications `p_ids` (none: all of
-- them, of `p_tenant` if given); how many.
CREATE FUNCTION app_mark_my_notifications_read(p_ids uuid[], p_tenant uuid) RETURNS integer
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    marked integer;
BEGIN
    UPDATE public.notification n SET read_at = now()
    WHERE n.account_id = app_current_account() AND n.read_at IS NULL
      AND n.tenant_id IN (SELECT public.app_my_notification_tenants())
      AND (p_ids IS NULL OR n.id = ANY (p_ids))
      AND (p_tenant IS NULL OR n.tenant_id = p_tenant);
    GET DIAGNOSTICS marked = ROW_COUNT;
    RETURN marked;
END
$$;
REVOKE EXECUTE ON FUNCTION app_mark_my_notifications_read(uuid[], uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_mark_my_notifications_read(uuid[], uuid) TO mlbpp_app;

-- The worker, in a tenant's transaction: what it needs of the recipients
-- `p_accounts` that are active members of the current tenant (the others
-- are left out): where to write, in which language, their preferences.
CREATE FUNCTION app_notification_recipients(p_accounts uuid[]) RETURNS TABLE (
    account_id uuid, email text, display_name text, locale text, preferences jsonb, daily_digest boolean
)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT a.id, a.email, a.display_name, coalesce(a.locale, t.default_locale),
           coalesce(s.preferences, '{}'), coalesce(s.daily_digest, false)
    FROM public.account a
    JOIN public.membership m ON m.account_id = a.id AND m.tenant_id = app_current_tenant() AND m.status = 'active'
    JOIN public.tenant t ON t.id = m.tenant_id
    LEFT JOIN public.notification_settings s ON s.account_id = a.id
    WHERE a.id = ANY (p_accounts)
$$;
REVOKE EXECUTE ON FUNCTION app_notification_recipients(uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_notification_recipients(uuid[]) TO mlbpp_app;

-- The worker, outside any tenant: the accounts that asked for the daily
-- digest and have notifications waiting for it, with their local date,
-- weekday (1 Monday … 7 Sunday) and hour (their time zone, else their
-- first organization's), and whether today's digest went already.
CREATE FUNCTION app_digest_candidates() RETURNS TABLE (
    account_id uuid, email text, display_name text, locale text,
    local_date date, local_weekday integer, local_hour integer, sent_today boolean
)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    WITH zoned AS (
        SELECT a.id, a.email, a.display_name, coalesce(a.locale, 'fr') AS locale, s.last_digest_on,
               now() AT TIME ZONE coalesce(
                   a.time_zone,
                   (SELECT t.time_zone FROM public.membership m JOIN public.tenant t ON t.id = m.tenant_id
                    WHERE m.account_id = a.id ORDER BY m.created_at LIMIT 1),
                   'UTC') AS local_now
        FROM public.notification_settings s
        JOIN public.account a ON a.id = s.account_id
        WHERE s.daily_digest AND a.erased_at IS NULL
          AND EXISTS (SELECT 1 FROM public.notification n
                      WHERE n.account_id = a.id AND n.email_wanted AND n.emailed_at IS NULL)
    )
    SELECT id, email, display_name, locale, local_now::date,
           extract(isodow FROM local_now)::integer, extract(hour FROM local_now)::integer,
           coalesce(last_digest_on >= local_now::date, false)
    FROM zoned
$$;
REVOKE EXECUTE ON FUNCTION app_digest_candidates() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_digest_candidates() TO mlbpp_app;

-- The worker: what the digest of `p_account` tells, oldest first, of its
-- active memberships.
CREATE FUNCTION app_digest_notifications(p_account uuid) RETURNS TABLE (
    id uuid, created_at timestamptz, tenant_slug text, tenant_name text, event text,
    actor_name text, details jsonb
)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT n.id, n.created_at, t.slug, t.name, n.event,
           public.app_member_name_in(n.tenant_id, n.actor_id), n.details
    FROM public.notification n
    JOIN public.tenant t ON t.id = n.tenant_id AND t.status = 'active'
    JOIN public.membership m ON m.tenant_id = n.tenant_id AND m.account_id = p_account AND m.status = 'active'
    WHERE n.account_id = p_account AND n.email_wanted AND n.emailed_at IS NULL
    ORDER BY n.created_at, n.id
$$;
REVOKE EXECUTE ON FUNCTION app_digest_notifications(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_digest_notifications(uuid) TO mlbpp_app;

-- The worker: the digest of `p_account` for its local `p_date` went with
-- the notifications `p_ids`.
CREATE FUNCTION app_digest_sent(p_account uuid, p_date date, p_ids uuid[]) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.notification SET emailed_at = now()
    WHERE account_id = p_account AND id = ANY (p_ids) AND emailed_at IS NULL;
    UPDATE public.notification_settings SET last_digest_on = p_date WHERE account_id = p_account;
END
$$;
REVOKE EXECUTE ON FUNCTION app_digest_sent(uuid, date, uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_digest_sent(uuid, date, uuid[]) TO mlbpp_app;
