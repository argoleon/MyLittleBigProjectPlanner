-- Keys of portfolios and programs (L06-US01): every node now carries a key,
-- unique in its tenant, so that queries name any node
-- (`project IN descendantsOf("PORTF-HR")`).
--
-- * A project keeps its key format (`PLAT`: 2 to 10 capitals and digits);
--   its tickets' keys derive from it, so it never changes.
-- * A portfolio or program takes 2 to 20 capitals, digits and single dashes
--   (`PORTF-HR`, `PROG-MODSI`); it may change.
--
-- Existing portfolios and programs get a key made of their type and name
-- (`PORTF-RESSOURCES-HUM`), for their administrators to review.

-- A key for a portfolio or program named `name`, free in the current tenant
-- (`app.tenant_id`). The tenant is filtered explicitly: the role running
-- migrations may bypass row-level security.
CREATE FUNCTION app_generated_node_key(node_type text, name text) RETURNS text
    LANGUAGE plpgsql
AS $$
DECLARE
    prefix    text := CASE node_type WHEN 'portfolio' THEN 'PORTF' ELSE 'PROG' END;
    base      text;
    candidate text;
    suffix    integer := 1;
BEGIN
    base := upper(translate(name,
        'ÀÁÂÃÄÅàáâãäåÇçÈÉÊËèéêëÌÍÎÏìíîïÑñÒÓÔÕÖòóôõöÙÚÛÜùúûüÝýÿŒœÆæ',
        'AAAAAAaaaaaaCcEEEEeeeeIIIIiiiiNnOOOOOoooooUUUUuuuuYyyOoAa'));
    base := btrim(regexp_replace(base, '[^A-Z0-9]+', '-', 'g'), '-');
    LOOP
        candidate := CASE WHEN base = '' THEN prefix ELSE prefix || '-' || base END;
        IF suffix > 1 THEN
            candidate := rtrim(left(candidate, 20 - length(suffix::text) - 1), '-') || '-' || suffix;
        ELSE
            candidate := rtrim(left(candidate, 20), '-');
        END IF;
        EXIT WHEN NOT EXISTS (
            SELECT 1 FROM node WHERE tenant_id = app_current_tenant() AND key = candidate);
        suffix := suffix + 1;
    END LOOP;
    RETURN candidate;
END
$$;
REVOKE EXECUTE ON FUNCTION app_generated_node_key(text, text) FROM PUBLIC;

ALTER TABLE node DROP CONSTRAINT node_key_check;
ALTER TABLE node DROP CONSTRAINT node_key_of_projects;

-- Each tenant in turn, in its context: row-level security is forced on
-- `node` for a migration role that does not bypass it.
DO $$
DECLARE
    t record;
    n record;
BEGIN
    FOR t IN SELECT id FROM tenant LOOP
        PERFORM set_config('app.tenant_id', t.id::text, true);
        FOR n IN
            SELECT id, node_type, name FROM node
            WHERE tenant_id = t.id AND key IS NULL
            ORDER BY created_at, id
        LOOP
            UPDATE node SET key = app_generated_node_key(n.node_type, n.name) WHERE id = n.id;
        END LOOP;
    END LOOP;
    PERFORM set_config('app.tenant_id', '', true);
END
$$;

ALTER TABLE node ALTER COLUMN key SET NOT NULL;
ALTER TABLE node ADD CONSTRAINT node_key_format CHECK (
    CASE node_type
        WHEN 'project' THEN key ~ '^[A-Z][A-Z0-9]{1,9}$'
        ELSE key ~ '^[A-Z][A-Z0-9]*(-[A-Z0-9]+)*$' AND length(key) BETWEEN 2 AND 20
    END
);
