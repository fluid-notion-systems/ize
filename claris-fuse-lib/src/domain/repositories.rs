use std::path::{Path, PathBuf};
use std::time::SystemTime;
use async_trait::async_trait;

use super::models::{FileChange, FileVersion, VersionedFile, VersionQuery, DomainError};

/// Result type for repository operations
pub type RepositoryResult<T> = Result<T, DomainError>;

/// Interface for accessing file versions
#[async_trait]
pub trait VersionRepository: Send + Sync {
    /// Initialize the repository
    async fn init(&self) -> RepositoryResult<()>;
    
    /// Close the repository and clean up resources
    async fn close(&self) -> RepositoryResult<()>;
    
    /// Record a new file version
    async fn save_version(&self, change: FileChange) -> RepositoryResult<u64>;
    
    /// Get all versions of a file
    async fn get_file_versions(&self, path: &Path) -> RepositoryResult<VersionedFile>;
    
    /// Get a specific version by ID
    async fn get_version(&self, version_id: u64) -> RepositoryResult<FileVersion>;
    
    /// Get the content of a specific version
    async fn get_version_content(&self, version_id: u64) -> RepositoryResult<Option<Vec<u8>>>;
    
    /// Get versions matching the query parameters
    async fn query_versions(&self, query: &VersionQuery) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Delete a specific version and its content
    async fn delete_version(&self, version_id: u64) -> RepositoryResult<()>;
    
    /// Update the description of a version
    async fn update_version_description(
        &self,
        version_id: u64,
        description: String,
    ) -> RepositoryResult<()>;
}

/// Interface for searchable version repositories
#[async_trait]
pub trait SearchableVersionRepository: VersionRepository {
    /// Search versions by description text
    async fn search_by_description(&self, query: &str) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Get versions created within a specific time range
    async fn get_versions_in_timerange(
        &self,
        since: SystemTime,
        until: SystemTime,
    ) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Get versions by operation type
    async fn get_versions_by_operation(
        &self,
        operation_type: super::models::OperationType,
    ) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Get versions for files matching a path pattern
    async fn get_versions_by_path_pattern(
        &self,
        pattern: &str,
    ) -> RepositoryResult<Vec<FileVersion>>;
}

/// Interface for file system operations
#[async_trait]
pub trait FileSystemRepository: Send + Sync {
    /// Initialize the file system repository
    async fn init(&self) -> RepositoryResult<()>;
    
    /// Close the file system repository
    async fn close(&self) -> RepositoryResult<()>;
    
    /// Read a file's content
    async fn read_file(&self, path: &Path) -> RepositoryResult<Vec<u8>>;
    
    /// Write content to a file
    async fn write_file(&self, path: &Path, content: &[u8]) -> RepositoryResult<()>;
    
    /// Delete a file
    async fn delete_file(&self, path: &Path) -> RepositoryResult<()>;
    
    /// Rename a file
    async fn rename_file(&self, from: &Path, to: &Path) -> RepositoryResult<()>;
    
    /// Check if a file exists
    async fn file_exists(&self, path: &Path) -> RepositoryResult<bool>;
    
    /// Get file metadata
    async fn get_metadata(&self, path: &Path) -> RepositoryResult<std::fs::Metadata>;
    
    /// List directory contents
    async fn list_directory(&self, path: &Path) -> RepositoryResult<Vec<PathBuf>>;
    
    /// Create a directory
    async fn create_directory(&self, path: &Path) -> RepositoryResult<()>;
    
    /// Remove a directory
    async fn remove_directory(&self, path: &Path) -> RepositoryResult<()>;
}

/// Factory for creating repositories
pub trait RepositoryFactory: Send + Sync {
    /// Create a version repository
    fn create_version_repository(&self) -> RepositoryResult<Box<dyn VersionRepository>>;
    
    /// Create a searchable version repository
    fn create_searchable_repository(&self) -> RepositoryResult<Box<dyn SearchableVersionRepository>>;
    
    /// Create a file system repository
    fn create_filesystem_repository(&self) -> RepositoryResult<Box<dyn FileSystemRepository>>;
}