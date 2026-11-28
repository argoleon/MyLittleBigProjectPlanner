-- The original estimate in hours (specification §6, L09-US03): a third
-- system field, on every type, which boards estimated in hours sum for
-- their totals and their sprints' committed and completed scope. Remaining
-- work stays for the burndown.
--
-- * New configurations get it with the other system fields (after them).
-- * The configurations made before get it from `app_add_original_estimate`,
--   run for every tenant: each published version at the end of its fields
--   and on its general creation and view screens; each draft with the same
--   identity, so that publishing it changes nothing; and the tickets with
--   remaining work but no original estimate get their remaining work as
--   original estimate, which hour boards summed until now.

CREATE OR REPLACE FUNCTION app_seed_default_fields(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant uuid;
    points uuid[];
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    SELECT coalesce(array_agg(item_id ORDER BY position), '{}') INTO points
    FROM public.issue_type WHERE version_id = p_version AND key IN ('story', 'task');
    INSERT INTO public.field_def
        (id, tenant_id, version_id, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position)
    VALUES
        (uuidv7(), tenant, p_version, uuidv7(), 'story_points', '{"fr": "Points", "en": "Points"}', 'number',
         '{"decimals": 1}', true, false, points, 0),
        (uuidv7(), tenant, p_version, uuidv7(), 'remaining', '{"fr": "Reste à faire", "en": "Remaining work"}', 'number',
         '{"decimals": 2, "unit": "hours"}', true, true, '{}', 1),
        (uuidv7(), tenant, p_version, uuidv7(), 'original_estimate',
         '{"fr": "Estimation initiale", "en": "Original estimate"}', 'number',
         '{"decimals": 2, "unit": "hours"}', true, true, '{}', 2);
END
$$;

-- The original estimate for the configurations of the current tenant that
-- have none; the number of published versions it was added to.
CREATE FUNCTION app_add_original_estimate() RETURNS integer
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    v record;
    item uuid;
    added integer := 0;
BEGIN
    FOR v IN
        SELECT p.id, p.config_id FROM public.project_config_version p
        WHERE p.tenant_id = public.app_current_tenant() AND p.state = 'published'
          AND NOT EXISTS (SELECT 1 FROM public.field_def f WHERE f.version_id = p.id AND f.key = 'original_estimate')
    LOOP
        item := uuidv7();
        -- The published version and its drafts, with the same identity.
        INSERT INTO public.field_def
            (id, tenant_id, version_id, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position)
        SELECT uuidv7(), d.tenant_id, d.id, item, 'original_estimate',
               '{"fr": "Estimation initiale", "en": "Original estimate"}', 'number', '{"decimals": 2, "unit": "hours"}',
               true, true, '{}', coalesce((SELECT max(f.position) + 1 FROM public.field_def f WHERE f.version_id = d.id), 0)
        FROM public.project_config_version d
        WHERE d.config_id = v.config_id AND d.state IN ('published', 'draft')
          AND NOT EXISTS (SELECT 1 FROM public.field_def f WHERE f.version_id = d.id AND f.key = 'original_estimate');
        -- On their general creation and view screens.
        UPDATE public.screen s SET field_ids = s.field_ids || item
        FROM public.project_config_version d
        WHERE s.version_id = d.id AND d.config_id = v.config_id AND d.state IN ('published', 'draft')
          AND s.issue_type_id IS NULL AND s.kind IN ('creation', 'view')
          AND NOT item = ANY (s.field_ids);
        -- Remaining work as the original estimate of the tickets that have none.
        UPDATE public.issue i
        SET fields = i.fields || jsonb_build_object(item::text, i.fields -> r.item_id::text)
        FROM public.node n, public.field_def r
        WHERE i.project_id = n.id AND n.config_id = v.config_id
          AND r.version_id = v.id AND r.key = 'remaining'
          AND jsonb_typeof(i.fields -> r.item_id::text) = 'number'
          AND NOT i.fields ? item::text;
        added := added + 1;
    END LOOP;
    RETURN added;
END
$$;
REVOKE EXECUTE ON FUNCTION app_add_original_estimate() FROM PUBLIC;

-- Every tenant, one at a time under its own context (row-level security).
DO $$
DECLARE
    t record;
BEGIN
    FOR t IN SELECT id FROM tenant LOOP
        PERFORM set_config('app.tenant_id', t.id::text, true);
        PERFORM app_add_original_estimate();
    END LOOP;
    PERFORM set_config('app.tenant_id', '', true);
END
$$;
