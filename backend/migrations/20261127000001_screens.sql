-- Screens of a project configuration (L04-US06). Specification §5.
--
-- Like the rest of a configuration, screens belong to a version and keep
-- `item_id` from one version to the next.
--
-- - A creation or view screen is general (`issue_type_id` null: one of
--   each per version, never removed), or for one type, which then uses it
--   instead of the general one; it goes with its type.
-- - A transition screen has a name and is attached to transitions
--   (`workflow_transition.screen_id`); it cannot be removed while one uses
--   it.
-- - `field_ids` lists fields of the version (`item_id`) in their order. The
--   field rules win when a screen is shown (domain::screens::resolve): a
--   hidden field is left out, a required one missing is added at the end.

CREATE TABLE screen (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    version_id    uuid NOT NULL,
    item_id       uuid NOT NULL,
    kind          text NOT NULL CHECK (kind IN ('creation', 'view', 'transition')),
    issue_type_id uuid,
    -- Translatable: {"fr": …, "en": …}, at least one.
    name          jsonb NOT NULL CHECK (jsonb_typeof(name) = 'object' AND name <> '{}'::jsonb),
    field_ids     uuid[] NOT NULL DEFAULT '{}',
    position      integer NOT NULL CHECK (position >= 0),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    version       bigint NOT NULL DEFAULT 1,
    CONSTRAINT screen_transition_has_no_type CHECK (kind <> 'transition' OR issue_type_id IS NULL),
    UNIQUE (version_id, item_id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES project_config_version (tenant_id, id) ON DELETE CASCADE,
    -- A type removed takes its screens.
    FOREIGN KEY (version_id, issue_type_id) REFERENCES issue_type (version_id, item_id) ON DELETE CASCADE
);
-- One general creation screen and one general view screen per version, and
-- at most one of each per type.
CREATE UNIQUE INDEX screen_kind_type_unique ON screen (version_id, kind, issue_type_id) NULLS NOT DISTINCT
    WHERE kind <> 'transition';
CREATE INDEX screen_tenant_version_idx ON screen (tenant_id, version_id, position);
SELECT app_setup_business_table('screen');
CREATE POLICY screen_platform ON screen TO mlbpp_owner USING (true) WITH CHECK (true);

-- The transition screen of a transition. Checked at commit: copying a
-- version copies the transitions before the screens.
ALTER TABLE workflow_transition ADD COLUMN screen_id uuid;
ALTER TABLE workflow_transition
    ADD CONSTRAINT workflow_transition_screen_fk FOREIGN KEY (version_id, screen_id)
    REFERENCES screen (version_id, item_id) DEFERRABLE INITIALLY DEFERRED;

-- The general screens of a version (domain::screens::DEFAULT_SCREENS; an
-- integration test keeps both in step), with its fields in their order.
-- Seeded after the fields.
CREATE FUNCTION app_seed_default_screens(p_version uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    tenant uuid;
    fields uuid[];
BEGIN
    SELECT tenant_id INTO tenant FROM public.project_config_version WHERE id = p_version;
    SELECT coalesce(array_agg(item_id ORDER BY position, key), '{}') INTO fields
    FROM public.field_def WHERE version_id = p_version;
    INSERT INTO public.screen (id, tenant_id, version_id, item_id, kind, issue_type_id, name, field_ids, position)
    VALUES
        (uuidv7(), tenant, p_version, uuidv7(), 'creation', NULL, '{"fr": "Création", "en": "Creation"}', fields, 0),
        (uuidv7(), tenant, p_version, uuidv7(), 'view', NULL, '{"fr": "Consultation", "en": "View"}', fields, 1);
END
$$;

CREATE OR REPLACE FUNCTION app_seed_default_config(p_tenant uuid) RETURNS uuid
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
DECLARE
    config  uuid := uuidv7();
    version uuid := uuidv7();
BEGIN
    INSERT INTO public.project_config (id, tenant_id, name, is_default)
    VALUES (config, p_tenant, '{"fr": "Configuration par défaut", "en": "Default configuration"}', true);
    INSERT INTO public.project_config_version (id, tenant_id, config_id, number, state, published_at)
    VALUES (version, p_tenant, config, 1, 'published', now());
    -- The workflow first: the types follow it; then the fields, on the
    -- types; then the screens, of the fields.
    PERFORM public.app_seed_default_workflow(version);
    PERFORM public.app_seed_default_issue_types(version);
    PERFORM public.app_seed_default_fields(version);
    PERFORM public.app_seed_default_screens(version);
    RETURN config;
END
$$;

-- The published versions made before get the general screens; their
-- drafts copy them, with the same identities and the fields they still
-- have.
SELECT app_seed_default_screens(v.id)
FROM project_config_version v
WHERE v.state = 'published'
  AND NOT EXISTS (SELECT 1 FROM screen s WHERE s.version_id = v.id);

INSERT INTO screen (id, tenant_id, version_id, item_id, kind, issue_type_id, name, field_ids, position)
SELECT uuidv7(), s.tenant_id, d.id, s.item_id, s.kind, NULL, s.name,
       coalesce(ARRAY(SELECT f.item_id FROM unnest(s.field_ids) WITH ORDINALITY AS o (item_id, n)
                      JOIN field_def f ON f.version_id = d.id AND f.item_id = o.item_id
                      ORDER BY o.n), '{}'),
       s.position
FROM project_config_version d
JOIN project_config_version p ON p.config_id = d.config_id AND p.state = 'published'
JOIN screen s ON s.version_id = p.id
WHERE d.state = 'draft'
  AND NOT EXISTS (SELECT 1 FROM screen x WHERE x.version_id = d.id);
