-- Calendars (specification §7, §8, L07-US04): which days resources work.
--
-- * Working weekdays come from the capacity (capacity_pattern, the tenant
--   default): calendars only hold dated exceptions and a set of public
--   holidays, computed by the application for any year (no stored rows).
-- * Every tenant has one `tenant` calendar (French holidays by default).
--   `site` calendars ("Paris", "Strasbourg") use their own set, or the
--   tenant's when `holiday_set` is NULL. A resource belongs to one site, or
--   to none (the tenant calendar then applies).
-- * Exceptions belong to a calendar or to a resource; the closest level
--   decides (resource, then site, then tenant).

CREATE TABLE calendar (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    kind        text NOT NULL CHECK (kind IN ('tenant', 'site')),
    -- Translatable: {"fr": …, "en": …}.
    name        jsonb NOT NULL CHECK (jsonb_typeof(name) = 'object' AND name <> '{}'::jsonb),
    -- fr, fr-alsace-moselle or none; NULL: the tenant's (sites only).
    holiday_set text CHECK (holiday_set IN ('fr', 'fr-alsace-moselle', 'none')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, id),
    CHECK (kind = 'site' OR holiday_set IS NOT NULL)
);
-- One tenant calendar per tenant.
CREATE UNIQUE INDEX calendar_tenant_idx ON calendar (tenant_id) WHERE kind = 'tenant';
CREATE UNIQUE INDEX calendar_tenant_name_fr_idx ON calendar (tenant_id, lower(name ->> 'fr'));
CREATE UNIQUE INDEX calendar_tenant_name_en_idx ON calendar (tenant_id, lower(name ->> 'en'));
SELECT app_setup_business_table('calendar');
CREATE POLICY calendar_platform ON calendar TO mlbpp_owner USING (true) WITH CHECK (true);

-- The site of a resource; none: the tenant calendar.
ALTER TABLE resource ADD COLUMN calendar_id uuid;
ALTER TABLE resource ADD CONSTRAINT resource_calendar_fkey
    FOREIGN KEY (tenant_id, calendar_id) REFERENCES calendar (tenant_id, id) ON DELETE SET NULL (calendar_id);
CREATE INDEX resource_calendar_idx ON resource (tenant_id, calendar_id) WHERE calendar_id IS NOT NULL;

CREATE TABLE calendar_exception (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    calendar_id uuid,
    resource_id uuid,
    -- A local date, never an instant.
    day         date NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('holiday', 'non_working', 'working')),
    label       text CHECK (label IS NULL OR (length(label) BETWEEN 1 AND 100 AND label = btrim(label))),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    -- Of a calendar, or of a resource.
    CHECK ((calendar_id IS NULL) <> (resource_id IS NULL)),
    FOREIGN KEY (tenant_id, calendar_id) REFERENCES calendar (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX calendar_exception_calendar_day_idx ON calendar_exception (tenant_id, calendar_id, day)
    WHERE calendar_id IS NOT NULL;
CREATE UNIQUE INDEX calendar_exception_resource_day_idx ON calendar_exception (tenant_id, resource_id, day)
    WHERE resource_id IS NOT NULL;
SELECT app_setup_business_table('calendar_exception');

-- The tenant calendar of `p_tenant`, with French holidays.
CREATE FUNCTION app_seed_default_calendar(p_tenant uuid) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    INSERT INTO public.calendar (id, tenant_id, kind, name, holiday_set)
    SELECT uuidv7(), p_tenant, 'tenant', '{"fr": "Organisation", "en": "Organization"}', 'fr'
    WHERE NOT EXISTS (SELECT 1 FROM public.calendar c WHERE c.tenant_id = p_tenant AND c.kind = 'tenant');
$$;
REVOKE EXECUTE ON FUNCTION app_seed_default_calendar(uuid) FROM PUBLIC;

CREATE OR REPLACE FUNCTION app_create_tenant_defaults() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
BEGIN
    -- A UUID v7 like every key: 48-bit milliseconds, version 7, variant 2.
    INSERT INTO public.tenant_auth_policy (id, tenant_id) VALUES (uuidv7(), NEW.id);
    PERFORM public.app_seed_default_roles(NEW.id);
    PERFORM public.app_seed_default_config(NEW.id);
    PERFORM public.app_seed_default_calendar(NEW.id);
    RETURN NEW;
END
$$;

SELECT app_seed_default_calendar(id) FROM tenant;
