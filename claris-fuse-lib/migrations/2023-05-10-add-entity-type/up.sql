-- Add entity_type to file_paths
ALTER TABLE file_paths RENAME TO paths_old;

-- Create new paths table with entity_type column
CREATE TABLE paths (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('file', 'directory')),
    created_at INTEGER NOT NULL,
    last_modified INTEGER NOT NULL
);

-- Copy data from old table to new table (default to 'file' for existing entries)
INSERT INTO paths (id, path, entity_type, created_at, last_modified)
SELECT id, path, 'file', created_at, last_modified FROM paths_old;

-- Update foreign keys in versions table
UPDATE versions SET file_path_id = file_path_id;

-- Create metadata table
CREATE TABLE metadata (
    path_id INTEGER PRIMARY KEY,
    mode INTEGER NOT NULL DEFAULT 644,
    uid INTEGER NOT NULL DEFAULT 1000,
    gid INTEGER NOT NULL DEFAULT 1000,
    atime INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    ctime INTEGER NOT NULL,
    FOREIGN KEY (path_id) REFERENCES paths(id) ON DELETE CASCADE
);

-- Add initial metadata for existing paths
INSERT INTO metadata (path_id, atime, mtime, ctime)
SELECT id, last_modified, last_modified, last_modified FROM paths;

-- Create indexes for the new table
CREATE INDEX idx_paths_path ON paths(path);
CREATE INDEX idx_paths_entity_type ON paths(entity_type);

-- Update triggers
DROP TRIGGER IF EXISTS update_file_last_modified;
CREATE TRIGGER update_path_last_modified
AFTER INSERT ON versions
BEGIN
    UPDATE paths
    SET last_modified = NEW.timestamp
    WHERE id = NEW.file_path_id;
END;

-- Clean up old table
DROP TABLE paths_old;