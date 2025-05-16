use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;

/// Enum representing the type of file operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationType {
    Create,
    Write,
    Truncate,
    Unlink,
    Rename,
    Mkdir,
    Rmdir,
    Symlink,
    Link,
    Chmod,
    Chown,
    Utimens,
    SetXattr,
    RemoveXattr,
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::Create => write!(f, "Create"),
            OperationType::Write => write!(f, "Write"),
            OperationType::Truncate => write!(f, "Truncate"),
            OperationType::Unlink => write!(f, "Unlink"),
            OperationType::Rename => write!(f, "Rename"),
            OperationType::Mkdir => write!(f, "Mkdir"),
            OperationType::Rmdir => write!(f, "Rmdir"),
            OperationType::Symlink => write!(f, "Symlink"),
            OperationType::Link => write!(f, "Link"),
            OperationType::Chmod => write!(f, "Chmod"),
            OperationType::Chown => write!(f, "Chown"),
            OperationType::Utimens => write!(f, "Utimens"),
            OperationType::SetXattr => write!(f, "SetXattr"),
            OperationType::RemoveXattr => write!(f, "RemoveXattr"),
        }
    }
}

impl std::str::FromStr for OperationType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Create" => Ok(OperationType::Create),
            "Write" => Ok(OperationType::Write),
            "Truncate" => Ok(OperationType::Truncate),
            "Unlink" => Ok(OperationType::Unlink),
            "Rename" => Ok(OperationType::Rename),
            "Mkdir" => Ok(OperationType::Mkdir),
            "Rmdir" => Ok(OperationType::Rmdir),
            "Symlink" => Ok(OperationType::Symlink),
            "Link" => Ok(OperationType::Link),
            "Chmod" => Ok(OperationType::Chmod),
            "Chown" => Ok(OperationType::Chown),
            "Utimens" => Ok(OperationType::Utimens),
            "SetXattr" => Ok(OperationType::SetXattr),
            "RemoveXattr" => Ok(OperationType::RemoveXattr),
            _ => Err(format!("Unknown operation type: {}", s)),
        }
    }
}

/// Struct representing a version of a file
#[derive(Debug, Clone)]
pub struct FileVersion {
    /// Unique identifier for this version
    pub id: i64,

    /// Path of the file relative to the mount point
    pub path: PathBuf,

    /// Type of operation that created this version
    pub operation_type: OperationType,

    /// When this version was created
    pub timestamp: SystemTime,

    /// Size of the file at this version
    pub size: u64,

    /// Content hash to identify content (for deduplication)
    pub content_hash: Option<String>,

    /// Optional AI-generated description of the change
    pub description: Option<String>,
}

/// Struct representing a versioned file
#[derive(Debug, Clone)]
pub struct VersionedFile {
    /// Path of the file relative to the mount point
    pub path: PathBuf,

    /// List of versions, sorted by timestamp (newest first)
    pub versions: Vec<FileVersion>,
}

/// Errors that can occur during storage operations
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Version not found: {0}")]
    VersionNotFound(i64),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// Result type for storage operations
pub type StorageResult<T> = Result<T, StorageError>;

/// Base storage trait defining common operations for any storage backend
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Initialize the storage backend
    async fn init(&self) -> StorageResult<()>;

    /// Close the storage backend and clean up resources
    async fn close(&self) -> StorageResult<()>;

    /// Get the name of the storage backend
    fn name(&self) -> &str;

    /// Get the version of the storage backend
    fn version(&self) -> &str;
}

/// Storage trait defining the operations required for version history
#[async_trait]
pub trait VersionStorage: StorageBackend {
    /// Record a new file version
    async fn record_version(
        &self,
        path: PathBuf,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
    ) -> StorageResult<i64>;
    
    /// Get all versions of a file
    async fn get_file_versions(&self, path: &Path) -> StorageResult<VersionedFile>;

    /// Get a specific version of a file
    async fn get_version(&self, version_id: i64) -> StorageResult<FileVersion>;

    /// Get the content of a specific version
    async fn get_version_content(&self, version_id: i64) -> StorageResult<Option<Vec<u8>>>;

    /// Get all versions with optional filters
    async fn get_versions(
        &self,
        path_prefix: Option<PathBuf>,
        since: Option<SystemTime>,
        until: Option<SystemTime>,
        operation_types: Option<Vec<OperationType>>,
    ) -> StorageResult<Vec<FileVersion>>;
}

/// Extension trait for storage backends that support searchable descriptions
#[async_trait]
pub trait SearchableStorage: VersionStorage {
    /// Search versions by description (when AI descriptions are available)
    async fn search_versions_by_description(&self, query: &str) -> StorageResult<Vec<FileVersion>>;

    /// Update the AI-generated description for a version
    async fn update_description(&self, version_id: i64, description: String) -> StorageResult<()>;
}

/// Factory trait for creating storage backends
pub trait StorageFactory {
    fn create_storage(&self) -> StorageResult<Box<dyn VersionStorage>>;
}

/// Factory trait for creating searchable storage backends
pub trait SearchableStorageFactory {
    fn create_searchable_storage(&self) -> StorageResult<Box<dyn SearchableStorage>>;
}
