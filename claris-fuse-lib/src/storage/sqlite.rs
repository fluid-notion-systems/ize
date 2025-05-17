use chrono::Utc;
use libc::{getgid, getuid};
use log::{debug, info};
use rusqlite::{Connection, Result as SqliteResult};
use std::io::{self, ErrorKind, Result as IoResult};
use std::path::{Path, PathBuf};

use super::Storage;

/// SQLite storage engine for the Claris-FUSE filesystem
pub struct SqliteStorage {
    #[allow(dead_code)]
    conn: Connection,
    #[allow(dead_code)]
    path: PathBuf,
}

impl SqliteStorage {
    /// Initialize a new SQLite database for Claris-FUSE
    pub fn init<P: AsRef<Path>>(path: P) -> IoResult<()> {
        let db_path = path.as_ref().join("claris-fuse.db");

        // Check if the database already exists
        if db_path.exists() {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                format!("Database already exists at {}", db_path.display()),
            ));
        }

        // Create the connection to the new database
        let mut conn = Connection::open(&db_path).map_err(|e| {
            io::Error::new(
                ErrorKind::Other,
                format!("Failed to create database: {}", e),
            )
        })?;

        // Initialize the database schema
        Self::create_schema(&mut conn).map_err(|e| {
            io::Error::new(ErrorKind::Other, format!("Failed to create schema: {}", e))
        })?;

        info!(
            "Initialized new Claris-FUSE database at {}",
            db_path.display()
        );
        Ok(())
    }

    /// Check if the database at the specified path is a valid Claris-FUSE database
    pub fn is_valid<P: AsRef<Path>>(path: P) -> IoResult<bool> {
        let db_path = path.as_ref().join("claris-fuse.db");

        // Check if the database file exists
        if !db_path.exists() {
            return Ok(false);
        }

        // Try to open the database and check if it has the expected schema
        match Connection::open(&db_path) {
            Ok(conn) => {
                let result = conn.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('directories', 'files', 'metadata', 'content')",
                    [],
                    |row| row.get::<_, i64>(0)
                );

                match result {
                    Ok(count) => Ok(count == 4), // We expect these 4 tables
                    Err(_) => Ok(false),
                }
            }
            Err(_) => Ok(false),
        }
    }

    /// Open an existing SQLite database
    pub fn open<P: AsRef<Path>>(path: P) -> IoResult<SqliteStorage> {
        let db_path = path.as_ref().join("claris-fuse.db");

        // Check if the database exists
        if !db_path.exists() {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                format!("Database not found at {}", db_path.display()),
            ));
        }

        // Open the database connection
        let conn = Connection::open(&db_path).map_err(|e| {
            io::Error::new(ErrorKind::Other, format!("Failed to open database: {}", e))
        })?;

        // Check if this is a valid Claris-FUSE database
        if !Self::is_valid(path.as_ref())? {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Not a valid Claris-FUSE database",
            ));
        }

        Ok(SqliteStorage {
            conn,
            path: db_path,
        })
    }

    /// Create the initial schema in the database
    fn create_schema(conn: &mut Connection) -> SqliteResult<()> {
        debug!("Creating SQLite database schema");

        // Start a transaction
        let tx = conn.transaction()?;

        // Create directories table
        tx.execute(
            "CREATE TABLE directories (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                metadata_id INTEGER NOT NULL
            )",
            [],
        )?;

        // Create files table
        tx.execute(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                directory_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                metadata_id INTEGER NOT NULL,
                UNIQUE(directory_id, name),
                FOREIGN KEY(directory_id) REFERENCES directories(id)
            )",
            [],
        )?;

        // Create metadata table
        tx.execute(
            "CREATE TABLE metadata (
                id INTEGER PRIMARY KEY,
                mode INTEGER NOT NULL,
                uid INTEGER NOT NULL,
                gid INTEGER NOT NULL,
                atime INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                ctime INTEGER NOT NULL
            )",
            [],
        )?;

        // Create content table
        tx.execute(
            "CREATE TABLE content (
                id INTEGER PRIMARY KEY,
                file_id INTEGER NOT NULL,
                data BLOB,
                FOREIGN KEY(file_id) REFERENCES files(id)
            )",
            [],
        )?;

        // Create root directory
        let now = Utc::now().timestamp();

        // Insert root directory metadata (typical directory permissions: rwxr-xr-x)
        tx.execute(
            "INSERT INTO metadata (id, mode, uid, gid, atime, mtime, ctime)
             VALUES (1, 493, ?, ?, ?, ?, ?)",
            rusqlite::params![
                unsafe { getuid() } as i32,
                unsafe { getgid() } as i32,
                now,
                now,
                now
            ],
        )?;

        // Insert root directory
        tx.execute(
            "INSERT INTO directories (id, path, created_at, metadata_id)
             VALUES (1, '/', ?, 1)",
            [now],
        )?;

        // Commit the transaction
        tx.commit()?;

        debug!("SQLite database schema created successfully");
        Ok(())
    }
}

impl Storage for SqliteStorage {
    fn write(&mut self, _path: &str, _data: &[u8]) -> IoResult<()> {
        // Not implemented yet
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "Not implemented yet",
        ))
    }

    fn read(&self, _path: &str) -> IoResult<Vec<u8>> {
        // Not implemented yet
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "Not implemented yet",
        ))
    }

    fn delete(&mut self, _path: &str) -> IoResult<()> {
        // Not implemented yet
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "Not implemented yet",
        ))
    }
}
