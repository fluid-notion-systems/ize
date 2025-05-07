use rusqlite::{Connection, Result};

pub struct SqliteSchema;

impl SqliteSchema {
    /// Initialize the SQLite database schema
    pub fn initialize(conn: &Connection) -> Result<()> {
        // Enable foreign keys support
        conn.execute("PRAGMA foreign_keys = ON;", [])?;

        // Create file_paths table to store unique file paths
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_paths (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                last_modified INTEGER NOT NULL
            )",
            [],
        )?;
        
        // Index on path for quick lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_file_paths_path ON file_paths(path);",
            [],
        )?;

        // Create versions table to track file versions
        conn.execute(
            "CREATE TABLE IF NOT EXISTS versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path_id INTEGER NOT NULL,
                operation_type TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                size INTEGER NOT NULL,
                content_hash TEXT,
                description TEXT,
                FOREIGN KEY (file_path_id) REFERENCES file_paths(id) ON DELETE CASCADE
            )",
            [],
        )?;
        
        // Indexes for common queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_versions_file_path_id ON versions(file_path_id);",
            [],
        )?;
        
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_versions_timestamp ON versions(timestamp);",
            [],
        )?;
        
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_versions_operation_type ON versions(operation_type);",
            [],
        )?;
        
        // Create full-text search index for description searches when using AI descriptions
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS versions_fts USING fts5(
                id UNINDEXED,
                description
            );",
            [],
        )?;
        
        // Create contents table to store file contents
        conn.execute(
            "CREATE TABLE IF NOT EXISTS contents (
                version_id INTEGER PRIMARY KEY,
                data BLOB,
                FOREIGN KEY (version_id) REFERENCES versions(id) ON DELETE CASCADE
            )",
            [],
        )?;
        
        // Create trigger to update file_paths.last_modified when versions are added
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS update_file_last_modified
            AFTER INSERT ON versions
            BEGIN
                UPDATE file_paths
                SET last_modified = NEW.timestamp
                WHERE id = NEW.file_path_id;
            END;",
            [],
        )?;
        
        // Create trigger to add entries to versions_fts when descriptions are added
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS update_versions_fts
            AFTER INSERT ON versions
            WHEN NEW.description IS NOT NULL
            BEGIN
                INSERT INTO versions_fts (id, description) VALUES (NEW.id, NEW.description);
            END;",
            [],
        )?;
        
        // Create trigger to update entries in versions_fts when descriptions are updated
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS update_versions_fts_description
            AFTER UPDATE OF description ON versions
            WHEN NEW.description IS NOT NULL
            BEGIN
                INSERT OR REPLACE INTO versions_fts (id, description) VALUES (NEW.id, NEW.description);
            END;",
            [],
        )?;
        
        // Create trigger to remove entries from versions_fts when versions are deleted
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS delete_versions_fts
            AFTER DELETE ON versions
            BEGIN
                DELETE FROM versions_fts WHERE id = OLD.id;
            END;",
            [],
        )?;
        
        // Create configuration table for retention policies
        conn.execute(
            "CREATE TABLE IF NOT EXISTS config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;
        
        // Insert default configuration
        conn.execute(
            "INSERT OR IGNORE INTO config (key, value) VALUES ('retention_days', '30')",
            [],
        )?;
        
        conn.execute(
            "INSERT OR IGNORE INTO config (key, value) VALUES ('max_versions_per_file', '100')",
            [],
        )?;
        
        Ok(())
    }
}