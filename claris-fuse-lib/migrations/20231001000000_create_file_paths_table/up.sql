-- Create file_paths table for tracking file paths
CREATE TABLE file_paths (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL, -- Timestamp in seconds since epoch
    last_modified INTEGER NOT NULL -- Timestamp in seconds since epoch
);

-- Create index for faster path lookups
CREATE INDEX idx_file_paths_path ON file_paths(path);

-- Add a comment to the table
PRAGMA user_version = 1;