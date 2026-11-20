-- Excel exports of list views (L06-US07): typed cells, the same pipeline as
-- CSV.
ALTER TABLE list_export DROP CONSTRAINT list_export_format_check;
ALTER TABLE list_export ADD CONSTRAINT list_export_format_check CHECK (format IN ('csv', 'xlsx'));
