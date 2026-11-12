-- Preferences of the account (L02-US08). Specification §3, §19.
--
-- The language (`locale`) was already there; the time zone in which the
-- account sees instants joins it. Both are NULL until the member picks
-- them: the tenant's defaults apply.

ALTER TABLE account ADD COLUMN time_zone text;

-- The time zone must be one PostgreSQL knows (IANA names): the application
-- checks it first, this keeps the table right whatever writes it.
CREATE FUNCTION app_check_account_time_zone() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.time_zone IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name = NEW.time_zone) THEN
        RAISE EXCEPTION 'unknown time zone %', NEW.time_zone USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER check_time_zone BEFORE INSERT OR UPDATE OF time_zone ON account
    FOR EACH ROW EXECUTE FUNCTION app_check_account_time_zone();
