-- Absences (specification §8, L07-US05): leave, training or other time off
-- of a resource, by whole or half days.
--
-- * From `start_date` to `end_date` included (local dates); it may start at
--   noon (the first morning is worked) and end at noon (the last afternoon
--   is worked). Absences of a resource never overlap, half day by half
--   day: the application checks it under a lock on the resource.
-- * `declared_by`: the member who declared it, the person or a manager.
-- * The comment is personal: only the person and the managers read it,
--   and it goes when the person is erased (in a tenant, or everywhere).

CREATE TABLE absence (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    resource_id    uuid NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('leave', 'training', 'other')),
    start_date     date NOT NULL,
    starts_at_noon boolean NOT NULL DEFAULT false,
    end_date       date NOT NULL,
    ends_at_noon   boolean NOT NULL DEFAULT false,
    comment        text CHECK (comment IS NULL OR (length(comment) BETWEEN 1 AND 500 AND comment = btrim(comment))),
    declared_by    uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    version        bigint NOT NULL DEFAULT 1,
    CHECK (end_date >= start_date AND end_date - start_date < 366),
    -- From noon to noon the same day would be nothing.
    CHECK (NOT (start_date = end_date AND starts_at_noon AND ends_at_noon)),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX absence_resource_start_idx ON absence (tenant_id, resource_id, start_date);
SELECT app_setup_business_table('absence');
-- The erasure triggers reach every tenant of an account.
CREATE POLICY absence_platform ON absence TO mlbpp_owner USING (true) WITH CHECK (true);

-- A member leaves the tenant: their resource stays, unlinked and inactive,
-- under the name it showed (L07-US01); erased there, the comments of its
-- absences go too.
CREATE OR REPLACE FUNCTION app_unlink_member_resource() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    v_pseudonym text := (SELECT 'Deleted user #' || e.pseudonym_number FROM public.erased_member e
                         WHERE e.tenant_id = OLD.tenant_id AND e.account_id = OLD.account_id);
BEGIN
    IF v_pseudonym IS NOT NULL THEN
        UPDATE public.absence a SET comment = NULL, version = a.version + 1
        FROM public.resource r
        WHERE a.resource_id = r.id AND r.tenant_id = OLD.tenant_id AND r.account_id = OLD.account_id
          AND a.comment IS NOT NULL;
    END IF;
    UPDATE public.resource r
    SET account_id = NULL,
        former_account_id = OLD.account_id,
        active = false,
        display_name = coalesce(
            v_pseudonym,
            (SELECT nullif(left(btrim(a.display_name), 200), '') FROM public.account a WHERE a.id = OLD.account_id),
            r.display_name),
        version = r.version + 1
    WHERE r.tenant_id = OLD.tenant_id AND r.account_id = OLD.account_id;
    RETURN OLD;
END
$$;

-- The global account is erased: the resources it left behind, in any
-- tenant, no longer keep its name, nor the comments of their absences.
CREATE OR REPLACE FUNCTION app_anonymize_former_resources() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.absence a SET comment = NULL, version = a.version + 1
    FROM public.resource r
    WHERE a.resource_id = r.id AND r.former_account_id = NEW.id AND a.comment IS NOT NULL;
    UPDATE public.resource
    SET display_name = 'Deleted user #' || NEW.pseudonym_number, version = version + 1
    WHERE former_account_id = NEW.id;
    RETURN NULL;
END
$$;
