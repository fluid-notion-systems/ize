-- Drop triggers first to avoid constraint errors
DROP TRIGGER IF EXISTS update_versions_fts;
DROP TRIGGER IF EXISTS update_versions_fts_description;
DROP TRIGGER IF EXISTS delete_versions_fts;

-- Drop the FTS virtual table
DROP TABLE IF EXISTS versions_fts;