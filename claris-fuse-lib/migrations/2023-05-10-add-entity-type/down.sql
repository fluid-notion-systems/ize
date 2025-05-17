-- Revert changes by creating a temporary table with the old structure
CREATE TABLE file_paths (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_modified INTEGER NOT NULL
);

-- Copy data from paths table to file_paths
INSERT INTO file_paths (id, path, created_at, last_modified)
SELECT id, path, created_at, last_modified FROM paths;

-- Update foreign keys
UPDATE versions SET file_path_id = file_path_id;

-- Drop new tables and triggers
DROP TRIGGER IF EXISTS update_path_last_modified;
DROP TABLE metadata;
DROP TABLE paths;

-- Recreate old trigger
CREATE TRIGGER update_file_last_modified
AFTER INSERT ON versions
BEGIN
    UPDATE file_paths
    SET last_modified = NEW.timestamp
    WHERE id = NEW.file_path_id;
END;