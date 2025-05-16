-- Create a virtual table for full-text searching of version descriptions
CREATE VIRTUAL TABLE versions_fts USING fts5(
    id UNINDEXED,
    description
);

-- Create trigger to add entries to versions_fts when descriptions are added
CREATE TRIGGER update_versions_fts
AFTER INSERT ON versions
WHEN NEW.description IS NOT NULL
BEGIN
    INSERT INTO versions_fts (id, description) VALUES (NEW.id, NEW.description);
END;

-- Create trigger to update entries in versions_fts when descriptions are updated
CREATE TRIGGER update_versions_fts_description
AFTER UPDATE OF description ON versions
WHEN NEW.description IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO versions_fts (id, description) VALUES (NEW.id, NEW.description);
END;

-- Create trigger to remove entries from versions_fts when versions are deleted
CREATE TRIGGER delete_versions_fts
AFTER DELETE ON versions
BEGIN
    DELETE FROM versions_fts WHERE id = OLD.id;
END;

-- Add a comment to indicate migration version
PRAGMA user_version = 4;