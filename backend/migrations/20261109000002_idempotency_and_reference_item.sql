-- Tenant resolution also returns the tenant's default language, used as the
-- fallback for translatable labels. Replaces the single-column resolver.
DROP FUNCTION app_resolve_tenant(text);

CREATE FUNCTION app_resolve_tenant(p_slug text)
    RETURNS TABLE (id uuid, default_locale text)
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.default_locale FROM public.tenant t WHERE t.slug = p_slug
$$;
REVOKE EXECUTE ON FUNCTION app_resolve_tenant(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_resolve_tenant(text) TO mlbpp_app;

-- Idempotency keys of creation requests (specification §15).
-- A replay with the same key returns the stored response instead of creating
-- a duplicate. Keys expire after 24 hours; an expired key can be reused.
-- Purge: infra::idempotency::purge_expired, run per tenant by the worker (L02).
CREATE TABLE idempotency_key (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL REFERENCES tenant (id),
    key             text NOT NULL CHECK (length(key) BETWEEN 1 AND 255),
    -- SHA-256 of the method, route and body: the same key with another
    -- request is refused.
    request_hash    bytea NOT NULL,
    -- NULL until the original request completes (same transaction).
    response_status smallint,
    response_body   jsonb,
    response_location text,
    response_etag   text,
    expires_at      timestamptz NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    version         bigint NOT NULL DEFAULT 1,
    CONSTRAINT idempotency_key_tenant_key_uq UNIQUE (tenant_id, key)
);
CREATE INDEX idempotency_key_tenant_expiry_idx ON idempotency_key (tenant_id, expires_at);
SELECT app_setup_business_table('idempotency_key');

-- Reference resource: the template every new endpoint copies
-- (docs/en/developer-guide/new-endpoint.md). Not a business feature.
CREATE TABLE reference_item (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id),
    code       text NOT NULL,
    -- Translatable label: {"fr": "…", "en": "…"}.
    label      jsonb NOT NULL CHECK (jsonb_typeof(label) = 'object'),
    quantity   integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    CONSTRAINT reference_item_tenant_code_uq UNIQUE (tenant_id, code)
);
-- Keyset pagination order: (created_at, id), always within one tenant.
CREATE INDEX reference_item_tenant_page_idx ON reference_item (tenant_id, created_at, id);
SELECT app_setup_business_table('reference_item');
