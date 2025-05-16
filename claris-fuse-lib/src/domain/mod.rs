// Domain module for the Claris FUSE filesystem
//
// This module contains the core domain models, repositories, and services
// that define the business logic for versioned file storage.

pub mod models;
pub mod repositories;
pub mod services;
pub mod standard_fs;

// Re-export commonly used types
pub use models::{
    DomainError, FileChange, FileMetadata, FileVersion, OperationType, VersionQuery, VersionedFile,
};
pub use repositories::{
    FileSystemRepository, RepositoryFactory, RepositoryResult, SearchableVersionRepository,
    VersionRepository,
};
pub use services::{
    DefaultSearchService, DefaultVersionService, SearchService, ServiceFactory, VersionService,
};
pub use standard_fs::StandardFileSystem;