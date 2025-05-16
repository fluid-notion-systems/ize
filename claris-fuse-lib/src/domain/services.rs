use std::path::Path;
use std::time::SystemTime;
use async_trait::async_trait;

use super::models::{
    FileChange, FileMetadata, FileVersion, VersionedFile, VersionQuery, OperationType,
};
use super::repositories::{VersionRepository, SearchableVersionRepository, RepositoryResult};

/// Service for managing file versions
#[async_trait]
pub trait VersionService: Send + Sync {
    /// Initialize the version service
    async fn init(&self) -> RepositoryResult<()>;
    
    /// Create a new version of a file
    async fn create_version(
        &self,
        path: impl AsRef<Path> + Send,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
        metadata: Option<FileMetadata>,
    ) -> RepositoryResult<u64>;
    
    /// Get all versions of a file
    async fn get_file_history(
        &self,
        path: impl AsRef<Path> + Send,
    ) -> RepositoryResult<VersionedFile>;
    
    /// Get a specific version by ID
    async fn get_version(&self, version_id: u64) -> RepositoryResult<FileVersion>;
    
    /// Get the content of a specific version
    async fn get_version_content(&self, version_id: u64) -> RepositoryResult<Option<Vec<u8>>>;
    
    /// Find versions matching query parameters
    async fn find_versions(&self, query: &VersionQuery) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Update a version's description
    async fn update_description(
        &self,
        version_id: u64,
        description: String,
    ) -> RepositoryResult<()>;
    
    /// Delete a version
    async fn delete_version(&self, version_id: u64) -> RepositoryResult<()>;
}

/// Default implementation of VersionService
pub struct DefaultVersionService {
    repository: Box<dyn VersionRepository>,
}

impl DefaultVersionService {
    /// Create a new version service with the given repository
    pub fn new(repository: Box<dyn VersionRepository>) -> Self {
        Self { repository }
    }
    
    /// Get the underlying repository
    pub fn repository(&self) -> &dyn VersionRepository {
        self.repository.as_ref()
    }
}

#[async_trait]
impl VersionService for DefaultVersionService {
    async fn init(&self) -> RepositoryResult<()> {
        self.repository.init().await
    }
    
    async fn create_version(
        &self,
        path: impl AsRef<Path> + Send,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
        metadata: Option<FileMetadata>,
    ) -> RepositoryResult<u64> {
        let change = FileChange {
            path: path.as_ref().to_path_buf(),
            operation_type,
            timestamp: SystemTime::now(),
            content,
            metadata,
            previous_path: None,
        };
        
        self.repository.save_version(change).await
    }
    
    async fn get_file_history(
        &self,
        path: impl AsRef<Path> + Send,
    ) -> RepositoryResult<VersionedFile> {
        self.repository.get_file_versions(path.as_ref()).await
    }
    
    async fn get_version(&self, version_id: u64) -> RepositoryResult<FileVersion> {
        self.repository.get_version(version_id).await
    }
    
    async fn get_version_content(&self, version_id: u64) -> RepositoryResult<Option<Vec<u8>>> {
        self.repository.get_version_content(version_id).await
    }
    
    async fn find_versions(&self, query: &VersionQuery) -> RepositoryResult<Vec<FileVersion>> {
        self.repository.query_versions(query).await
    }
    
    async fn update_description(
        &self,
        version_id: u64,
        description: String,
    ) -> RepositoryResult<()> {
        self.repository.update_version_description(version_id, description).await
    }
    
    async fn delete_version(&self, version_id: u64) -> RepositoryResult<()> {
        self.repository.delete_version(version_id).await
    }
}

/// Service for searching file versions
#[async_trait]
pub trait SearchService: Send + Sync {
    /// Search for versions based on description text
    async fn search_by_text(&self, query: &str) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Find versions created within a time period
    async fn find_versions_by_time(
        &self,
        since: SystemTime,
        until: Option<SystemTime>,
    ) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Find versions by operation type
    async fn find_versions_by_operation(
        &self,
        operation_type: OperationType,
    ) -> RepositoryResult<Vec<FileVersion>>;
    
    /// Find versions matching a path pattern
    async fn find_versions_by_path_pattern(
        &self,
        pattern: &str,
    ) -> RepositoryResult<Vec<FileVersion>>;
}

/// Default implementation of SearchService
pub struct DefaultSearchService {
    repository: Box<dyn SearchableVersionRepository>,
}

impl DefaultSearchService {
    /// Create a new search service with the given repository
    pub fn new(repository: Box<dyn SearchableVersionRepository>) -> Self {
        Self { repository }
    }
    
    /// Get the underlying repository
    pub fn repository(&self) -> &dyn SearchableVersionRepository {
        self.repository.as_ref()
    }
}

#[async_trait]
impl SearchService for DefaultSearchService {
    async fn search_by_text(&self, query: &str) -> RepositoryResult<Vec<FileVersion>> {
        self.repository.search_by_description(query).await
    }
    
    async fn find_versions_by_time(
        &self,
        since: SystemTime,
        until: Option<SystemTime>,
    ) -> RepositoryResult<Vec<FileVersion>> {
        let until = until.unwrap_or_else(SystemTime::now);
        self.repository.get_versions_in_timerange(since, until).await
    }
    
    async fn find_versions_by_operation(
        &self,
        operation_type: OperationType,
    ) -> RepositoryResult<Vec<FileVersion>> {
        self.repository.get_versions_by_operation(operation_type).await
    }
    
    async fn find_versions_by_path_pattern(
        &self,
        pattern: &str,
    ) -> RepositoryResult<Vec<FileVersion>> {
        self.repository.get_versions_by_path_pattern(pattern).await
    }
}

/// A factory for creating domain services
pub trait ServiceFactory: Send + Sync {
    /// Create a version service
    fn create_version_service(&self) -> DefaultVersionService;
    
    /// Create a search service
    fn create_search_service(&self) -> Box<dyn SearchService>;
}