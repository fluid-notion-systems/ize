mod traits;
mod sqlite_schema;
mod sqlite;

pub use traits::{
    FileVersion, OperationType, StorageError, StorageFactory, StorageResult, VersionStorage,
    VersionedFile, StorageBackend, SearchableStorage, SearchableStorageFactory,
};

pub use sqlite_schema::SqliteSchema;
pub use sqlite::{SqliteStorage, SqliteStorageFactory};