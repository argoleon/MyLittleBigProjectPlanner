-- Full-text search of tickets (specification §6, L06-US03): `text ~`,
-- `summary ~` and `description ~` of the query language.
--
-- * Multilingual: every text is indexed both with French and with English
--   stemming, so a word matches in either language; accents are ignored
--   (`unaccent`). Titles weigh more than descriptions (weights A and B).
-- * Comments have their own index: the search reads public comments only
--   for the portal, all of them for members.
-- * Typos: a word the tenant never wrote is corrected to the words it wrote
--   (`search_word`) within a small edit distance: candidates are picked
--   here by length and Levenshtein distance (`fuzzystrmatch`), then the
--   application keeps those within its exact rule (crates/domain).
--
-- Both extensions are "trusted": the database owner creates them.

CREATE EXTENSION IF NOT EXISTS unaccent;
CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;

-- French and English stemming, and plain words (the word list), all without
-- accents.
CREATE TEXT SEARCH CONFIGURATION mlbpp_fr (COPY = french);
ALTER TEXT SEARCH CONFIGURATION mlbpp_fr
    ALTER MAPPING FOR hword, hword_part, word WITH unaccent, french_stem;
CREATE TEXT SEARCH CONFIGURATION mlbpp_en (COPY = english);
ALTER TEXT SEARCH CONFIGURATION mlbpp_en
    ALTER MAPPING FOR hword, hword_part, word WITH unaccent, english_stem;
CREATE TEXT SEARCH CONFIGURATION mlbpp_simple (COPY = simple);
ALTER TEXT SEARCH CONFIGURATION mlbpp_simple
    ALTER MAPPING FOR hword, hword_part, word WITH unaccent, simple;

-- The text of a rich text document (Tiptap JSON): its text runs, in order
-- (`strict`: in `lax` mode `**` visits each run twice),
-- separated by spaces; empty for none.
CREATE FUNCTION app_rich_text(doc jsonb) RETURNS text
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN COALESCE(
    (SELECT string_agg(run #>> '{}', ' ') FROM jsonb_path_query(doc, 'strict $.**.text') AS run),
    '');

-- `text` indexed in both languages.
CREATE FUNCTION app_text_vector(text text) RETURNS tsvector
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN to_tsvector('mlbpp_fr', text) || to_tsvector('mlbpp_en', text);

ALTER TABLE issue ADD COLUMN search_vector tsvector GENERATED ALWAYS AS (
    setweight(app_text_vector(summary), 'A') || setweight(app_text_vector(app_rich_text(description)), 'B')
) STORED;
CREATE INDEX issue_search_vector_idx ON issue USING gin (search_vector);

ALTER TABLE comment ADD COLUMN search_vector tsvector GENERATED ALWAYS AS (
    app_text_vector(app_rich_text(content))
) STORED;
CREATE INDEX comment_search_vector_idx ON comment USING gin (search_vector);

-- The words of each tenant (titles, descriptions, comments), without
-- accents, in lower case: what a misspelt word is corrected to. Letters
-- only, 4 to 64 of them (shorter words are never corrected). A word stays
-- once written: a correction to a word no ticket holds any more finds
-- nothing, and the list itself is never shown.
CREATE TABLE search_word (
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    word       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, word)
);
-- Candidates are words of about the same length.
CREATE INDEX search_word_length_idx ON search_word (tenant_id, char_length(word));
SELECT app_setup_business_table('search_word');

-- Adds the words of `text` to the list of `tenant`.
CREATE FUNCTION app_remember_words(tenant uuid, text text) RETURNS void
    LANGUAGE sql
BEGIN ATOMIC
    INSERT INTO search_word (tenant_id, word)
    SELECT tenant, w.lexeme
    FROM unnest(to_tsvector('mlbpp_simple', text)) AS w
    WHERE w.lexeme ~ '^[[:alpha:]]{4,64}$'
    ON CONFLICT DO NOTHING;
END;

CREATE FUNCTION app_issue_words() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM app_remember_words(NEW.tenant_id, NEW.summary || ' ' || app_rich_text(NEW.description));
    RETURN NULL;
END
$$;
CREATE TRIGGER remember_words AFTER INSERT OR UPDATE OF summary, description ON issue
    FOR EACH ROW EXECUTE FUNCTION app_issue_words();

CREATE FUNCTION app_comment_words() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.content IS NOT NULL THEN
        PERFORM app_remember_words(NEW.tenant_id, app_rich_text(NEW.content));
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER remember_words AFTER INSERT OR UPDATE OF content ON comment
    FOR EACH ROW EXECUTE FUNCTION app_comment_words();

-- The words already written, each tenant in its context: row-level
-- security is forced on these tables for a migration role that does not
-- bypass it.
DO $$
DECLARE
    t record;
BEGIN
    FOR t IN SELECT id FROM tenant LOOP
        PERFORM set_config('app.tenant_id', t.id::text, true);
        PERFORM app_remember_words(t.id, i.summary || ' ' || app_rich_text(i.description))
        FROM issue i WHERE i.tenant_id = t.id;
        PERFORM app_remember_words(t.id, app_rich_text(c.content))
        FROM comment c WHERE c.tenant_id = t.id AND c.content IS NOT NULL;
    END LOOP;
    PERFORM set_config('app.tenant_id', '', true);
END
$$;
