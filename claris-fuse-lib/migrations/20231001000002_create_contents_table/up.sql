-- Create contents table for storing version content
CREATE TABLE contents (
    version_id INTEGER PRIMARY KEY,
    data BLOB,
    FOREIGN KEY (version_id) REFERENCES versions(id) ON DELETE CASCADE
);

-- Add a comment to indicate migration version
PRAGMA user_version = 3;