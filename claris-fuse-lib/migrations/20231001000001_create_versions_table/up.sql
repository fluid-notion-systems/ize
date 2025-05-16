-- Create versions table for tracking file versions
CREATE TABLE versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path_id INTEGER NOT NULL,
    operation_type TEXT NOT NULL,
    timestamp INTEGER NOT NULL, -- Timestamp in seconds since epoch
    size INTEGER NOT NULL,
    content_hash TEXT,
    description TEXT,
    FOREIGN KEY (file_path_id) REFERENCES file_paths(id) ON DELETE CASCADE
);

-- Create indexes for faster queries
CREATE INDEX idx_versions_file_path_id ON versions(file_path_id);
CREATE INDEX idx_versions_timestamp ON versions(timestamp);
CREATE INDEX idx_versions_operation_type ON versions(operation_type);

-- Create trigger to update file_paths.last_modified when versions are added
CREATE TRIGGER update_file_last_modified
AFTER INSERT ON versions
BEGIN
    UPDATE file_paths
    SET last_modified = NEW.timestamp
    WHERE id = NEW.file_path_id;
END;

-- Add a comment to the table
PRAGMA user_version = 2;