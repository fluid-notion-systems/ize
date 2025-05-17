pub mod diesel_sqlite;
pub mod models;
mod traits;

pub use traits::{
    FileVersion, OperationType, SearchableStorage, SearchableStorageFactory, StorageBackend,
    StorageError, StorageFactory, StorageResult, VersionStorage, VersionedFile,
};
