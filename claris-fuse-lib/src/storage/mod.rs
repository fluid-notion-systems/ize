pub mod sqlite;

use std::io::Result;
use std::path::Path;

/// A storage engine for the Claris-FUSE filesystem
pub trait Storage {
    /// Write data to storage
    fn write(&mut self, path: &str, data: &[u8]) -> Result<()>;

    /// Read data from storage
    fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Delete data from storage
    fn delete(&mut self, path: &str) -> Result<()>;
}

// Implementation of the Storage trait for initialization
pub struct StorageManager;

impl StorageManager {
    /// Initialize a new database with the specified storage engine
    pub fn init<P: AsRef<Path>>(path: P) -> Result<()> {
        sqlite::SqliteStorage::init(path)
    }

    /// Check if the database at the specified path is valid
    pub fn is_valid<P: AsRef<Path>>(path: P) -> Result<bool> {
        sqlite::SqliteStorage::is_valid(path)
    }

    /// Open an existing database at the specified path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<sqlite::SqliteStorage> {
        sqlite::SqliteStorage::open(path)
    }
}
