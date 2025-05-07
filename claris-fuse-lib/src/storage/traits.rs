use std::path::PathBuf;
use std::time::SystemTime;
use async_trait::async_trait;
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
}

/// Result type for storage operations
pub type StorageResult<T> = Result<T, StorageError>;

/// Storage trait defining the operations required for version history
#[async_trait]
pub trait VersionStorage: Send + Sync {
    /// Initialize storage backend
    async fn init(&self) -> StorageResult<()>;
    
    /// Record a new file version
    async fn record_version(
        &self,
        path: PathBuf,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
    ) -> StorageResult<i64>;
    
    /// Get all versions of a file
    async fn get_file_versions(&self, path: &PathBuf) -> StorageResult<VersionedFile>;
    
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
    
    /// Search versions by description (when AI descriptions are available)
    async fn search_versions_by_description(&self, query: &str) -> StorageResult<Vec<FileVersion>>;
    
    /// Update the AI-generated description for a version
    async fn update_description(&self, version_id: i64, description: String) -> StorageResult<()>;
}

/// Factory trait for creating storage backends
pub trait StorageFactory {
    fn create_storage(&self) -> Box<dyn VersionStorage>;
}