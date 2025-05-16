mod traits;
mod sqlite_schema;
mod sqlite;
pub mod diesel_sqlite;
pub mod models;

pub use traits::{
    FileVersion, OperationType, StorageError, StorageFactory, StorageResult, VersionStorage,
    VersionedFile, StorageBackend, SearchableStorage, SearchableStorageFactory,
};

pub use sqlite_schema::SqliteSchema;
pub use sqlite::{SqliteStorage, SqliteStorageFactory};