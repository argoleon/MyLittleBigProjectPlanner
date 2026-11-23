-- Resources (specification §8, L07-US01): what work is planned on, apart
-- from accounts.
--
-- * `kind`: `person` (with or without an account: a subcontractor who never
--   signs in) or `generic` (an unnamed role, "Rust developer"). Equipment
--   and rooms come after the MVP, as new kinds.
-- * A person may be linked to a member of the tenant (`account_id`), one
--   resource per member. While linked, it shows the member's live name
--   (`app_member_name`); `display_name` keeps a copy.
-- * When the member leaves the tenant, the resource is unlinked
--   (`former_account_id`), deactivated, and keeps the name they had (or
--   their pseudonym, erased in this tenant). Erasing the global account
--   later replaces that copy by its pseudonym, in every tenant.
-- * Resources are deactivated, never deleted: assignments (L08) refer to
--   them.
--
-- The new tenant role `resource_manager` manages them, as the tenant admin
-- does; every internal member reads them.

ALTER TABLE membership DROP CONSTRAINT membership_tenant_role_check;
ALTER TABLE membership ADD CONSTRAINT membership_tenant_role_check
    CHECK (tenant_role IN ('member', 'configuration_admin', 'resource_manager', 'tenant_admin'));
ALTER TABLE invitation DROP CONSTRAINT invitation_tenant_role_check;
ALTER TABLE invitation ADD CONSTRAINT invitation_tenant_role_check
    CHECK (tenant_role IN ('member', 'configuration_admin', 'resource_manager', 'tenant_admin'));

CREATE TABLE resource (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    kind              text NOT NULL CHECK (kind IN ('person', 'generic')),
    display_name      text NOT NULL
                      CHECK (length(display_name) BETWEEN 1 AND 200 AND display_name = btrim(display_name)),
    -- Free text shown next to the name ("Back-end developer").
    job_title         text
                      CHECK (job_title IS NULL OR (length(job_title) BETWEEN 1 AND 100 AND job_title = btrim(job_title))),
    -- The member this person is, in the same tenant.
    account_id        uuid,
    -- The member this person was, once they left the tenant.
    former_account_id uuid REFERENCES account (id) ON DELETE SET NULL,
    active            boolean NOT NULL DEFAULT true,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    version           bigint NOT NULL DEFAULT 1,
    -- Target of tenant-checked foreign keys (teams, assignments).
    UNIQUE (tenant_id, id),
    -- Only a person is, or was, a member.
    CHECK (kind = 'person' OR (account_id IS NULL AND former_account_id IS NULL)),
    CHECK (account_id IS NULL OR former_account_id IS NULL),
    -- A member of this tenant. `app_unlink_member_resource` unlinks the
    -- resource before the membership goes; the foreign key is the safety net.
    FOREIGN KEY (tenant_id, account_id) REFERENCES membership (tenant_id, account_id)
);
-- One resource per member.
CREATE UNIQUE INDEX resource_account_idx ON resource (tenant_id, account_id) WHERE account_id IS NOT NULL;
CREATE INDEX resource_former_account_idx ON resource (former_account_id) WHERE former_account_id IS NOT NULL;
CREATE INDEX resource_tenant_created_idx ON resource (tenant_id, created_at, id);
SELECT app_setup_business_table('resource');
-- The two triggers below reach every tenant of an account.
CREATE POLICY resource_platform ON resource TO mlbpp_owner USING (true) WITH CHECK (true);

-- A member leaves the tenant (removed, erased there or everywhere): their
-- resource stays, unlinked and inactive, under the name it showed.
CREATE FUNCTION app_unlink_member_resource() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.resource r
    SET account_id = NULL,
        former_account_id = OLD.account_id,
        active = false,
        display_name = coalesce(
            (SELECT 'Deleted user #' || e.pseudonym_number FROM public.erased_member e
             WHERE e.tenant_id = OLD.tenant_id AND e.account_id = OLD.account_id),
            (SELECT nullif(left(btrim(a.display_name), 200), '') FROM public.account a WHERE a.id = OLD.account_id),
            r.display_name),
        version = r.version + 1
    WHERE r.tenant_id = OLD.tenant_id AND r.account_id = OLD.account_id;
    RETURN OLD;
END
$$;
REVOKE EXECUTE ON FUNCTION app_unlink_member_resource() FROM PUBLIC;
CREATE TRIGGER unlink_resource BEFORE DELETE ON membership
    FOR EACH ROW EXECUTE FUNCTION app_unlink_member_resource();

-- The global account is erased: the resources it left behind, in any
-- tenant, no longer keep its name.
CREATE FUNCTION app_anonymize_former_resources() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.resource
    SET display_name = 'Deleted user #' || NEW.pseudonym_number, version = version + 1
    WHERE former_account_id = NEW.id;
    RETURN NULL;
END
$$;
REVOKE EXECUTE ON FUNCTION app_anonymize_former_resources() FROM PUBLIC;
CREATE TRIGGER anonymize_former_resources AFTER UPDATE OF erased_at ON account
    FOR EACH ROW WHEN (OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL)
    EXECUTE FUNCTION app_anonymize_former_resources();
