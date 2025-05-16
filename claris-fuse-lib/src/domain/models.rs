use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::convert::TryFrom;
use thiserror::Error;

/// Domain error types for handling errors in the domain layer
#[derive(Error, Debug)]
pub enum DomainError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    
    #[error("Version not found: {0}")]
    VersionNotFound(u64),
    
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),
    
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// The type of operation that was performed on a file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationType {
    /// The file was created
    Create,
    /// The file was written to
    Write,
    /// The file was deleted
    Delete,
    /// The file was renamed
    Rename,
    /// The file was truncated
    Truncate,
    /// A directory was created
    Mkdir,
    /// A directory was removed
    Rmdir,
    /// A symbolic link was created
    Symlink,
    /// A hard link was created
    Link,
    /// File permissions were changed
    Chmod,
    /// File ownership was changed
    Chown,
    /// File times were changed
    Utimens,
    /// Extended attributes were set
    SetXattr,
    /// Extended attributes were removed
    RemoveXattr,
}

impl OperationType {
    /// Returns true if this operation type typically has associated content
    pub fn has_content(&self) -> bool {
        matches!(self, 
            OperationType::Create | 
            OperationType::Write |
            OperationType::Truncate
        )
    }
    
    /// Returns true if this operation type changes file metadata
    pub fn changes_metadata(&self) -> bool {
        matches!(self, 
            OperationType::Chmod | 
            OperationType::Chown |
            OperationType::Utimens |
            OperationType::SetXattr |
            OperationType::RemoveXattr
        )
    }
    
    /// Returns the string representation of this operation type
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationType::Create => "Create",
            OperationType::Write => "Write",
            OperationType::Delete => "Delete",
            OperationType::Rename => "Rename",
            OperationType::Truncate => "Truncate",
            OperationType::Mkdir => "Mkdir",
            OperationType::Rmdir => "Rmdir",
            OperationType::Symlink => "Symlink",
            OperationType::Link => "Link",
            OperationType::Chmod => "Chmod",
            OperationType::Chown => "Chown",
            OperationType::Utimens => "Utimens",
            OperationType::SetXattr => "SetXattr",
            OperationType::RemoveXattr => "RemoveXattr",
        }
    }
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl TryFrom<&str> for OperationType {
    type Error = DomainError;
    
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "Create" => Ok(OperationType::Create),
            "Write" => Ok(OperationType::Write),
            "Delete" => Ok(OperationType::Delete),
            "Rename" => Ok(OperationType::Rename),
            "Truncate" => Ok(OperationType::Truncate),
            "Mkdir" => Ok(OperationType::Mkdir),
            "Rmdir" => Ok(OperationType::Rmdir),
            "Symlink" => Ok(OperationType::Symlink),
            "Link" => Ok(OperationType::Link),
            "Chmod" => Ok(OperationType::Chmod),
            "Chown" => Ok(OperationType::Chown),
            "Utimens" => Ok(OperationType::Utimens),
            "SetXattr" => Ok(OperationType::SetXattr),
            "RemoveXattr" => Ok(OperationType::RemoveXattr),
            _ => Err(DomainError::InvalidOperation(s.to_string())),
        }
    }
}

/// Represents file metadata at a specific version
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Size of the file in bytes
    pub size: u64,
    
    /// File mode/permissions
    pub mode: u32,
    
    /// Owner user ID
    pub uid: u32,
    
    /// Owner group ID
    pub gid: u32,
    
    /// Last access time
    pub atime: SystemTime,
    
    /// Last modification time
    pub mtime: SystemTime,
    
    /// Last status change time
    pub ctime: SystemTime,
}

impl Default for FileMetadata {
    fn default() -> Self {
        let now = SystemTime::now();
        Self {
            size: 0,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }
}

/// Represents a version of a file
#[derive(Debug, Clone)]
pub struct FileVersion {
    /// Unique identifier for this version
    pub id: u64,
    
    /// Path to the file at the time this version was created
    pub path: PathBuf,
    
    /// Type of operation that created this version
    pub operation_type: OperationType,
    
    /// Timestamp when this version was created
    pub timestamp: SystemTime,
    
    /// Size of the file contents in bytes
    pub size: u64,
    
    /// Optional hash of the file contents
    pub content_hash: Option<String>,
    
    /// Optional description of this version
    pub description: Option<String>,
    
    /// Optional metadata for this version
    pub metadata: Option<FileMetadata>,
}

impl FileVersion {
    /// Creates a new file version with minimal information
    pub fn new(
        id: u64,
        path: impl AsRef<Path>,
        operation_type: OperationType,
        timestamp: SystemTime,
        size: u64,
    ) -> Self {
        Self {
            id,
            path: path.as_ref().to_path_buf(),
            operation_type,
            timestamp,
            size,
            content_hash: None,
            description: None,
            metadata: None,
        }
    }
    
    /// Returns the elapsed time since this version was created
    pub fn age(&self) -> std::time::Duration {
        SystemTime::now()
            .duration_since(self.timestamp)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
    }
    
    /// Returns true if this version is from today
    pub fn is_today(&self) -> bool {
        let now = SystemTime::now();
        let today_start = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() / 86400 * 86400;
        
        let version_secs = self.timestamp
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        version_secs >= today_start
    }
    
    /// Sets the content hash for this version
    pub fn with_content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }
    
    /// Sets the description for this version
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
    
    /// Sets the metadata for this version
    pub fn with_metadata(mut self, metadata: FileMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// A collection of versions for a file
#[derive(Debug, Clone)]
pub struct VersionedFile {
    /// Path to the file
    pub path: PathBuf,
    
    /// List of versions, usually sorted by timestamp (newest first)
    pub versions: Vec<FileVersion>,
}

impl VersionedFile {
    /// Creates a new versioned file with the given path
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            versions: Vec::new(),
        }
    }
    
    /// Creates a new versioned file with the given path and versions
    pub fn with_versions(path: impl AsRef<Path>, versions: Vec<FileVersion>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            versions,
        }
    }
    
    /// Adds a version to this file
    pub fn add_version(&mut self, version: FileVersion) {
        self.versions.push(version);
        
        // Keep versions sorted by timestamp (newest first)
        self.versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    }
    
    /// Returns the latest version of this file, if any
    pub fn latest_version(&self) -> Option<&FileVersion> {
        self.versions.first()
    }
    
    /// Returns the number of versions
    pub fn version_count(&self) -> usize {
        self.versions.len()
    }
    
    /// Returns true if this file has no versions
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }
    
    /// Returns all versions created after the given timestamp
    pub fn versions_since(&self, timestamp: SystemTime) -> Vec<&FileVersion> {
        self.versions
            .iter()
            .filter(|v| v.timestamp >= timestamp)
            .collect()
    }
    
    /// Returns all versions of a specific operation type
    pub fn versions_by_operation(&self, operation_type: OperationType) -> Vec<&FileVersion> {
        self.versions
            .iter()
            .filter(|v| v.operation_type == operation_type)
            .collect()
    }
}

/// Represents a file change operation
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path to the file
    pub path: PathBuf,
    
    /// Type of operation
    pub operation_type: OperationType,
    
    /// Timestamp when the operation occurred
    pub timestamp: SystemTime,
    
    /// File content, if applicable to the operation type
    pub content: Option<Vec<u8>>,
    
    /// File metadata, if applicable
    pub metadata: Option<FileMetadata>,
    
    /// Optional previous path, for rename operations
    pub previous_path: Option<PathBuf>,
}

impl FileChange {
    /// Creates a new file change with minimal information
    pub fn new(
        path: impl AsRef<Path>,
        operation_type: OperationType,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            operation_type,
            timestamp,
            content: None,
            metadata: None,
            previous_path: None,
        }
    }
    
    /// Sets the content for this change
    pub fn with_content(mut self, content: Vec<u8>) -> Self {
        self.content = Some(content);
        self
    }
    
    /// Sets the metadata for this change
    pub fn with_metadata(mut self, metadata: FileMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
    
    /// Sets the previous path for this change (for rename operations)
    pub fn with_previous_path(mut self, previous_path: impl AsRef<Path>) -> Self {
        self.previous_path = Some(previous_path.as_ref().to_path_buf());
        self
    }
    
    /// Returns true if this change has content
    pub fn has_content(&self) -> bool {
        self.content.is_some()
    }
    
    /// Returns the size of the content, or 0 if no content
    pub fn content_size(&self) -> u64 {
        self.content.as_ref().map(|c| c.len() as u64).unwrap_or(0)
    }
}

/// A query for searching file versions
#[derive(Debug, Clone, Default)]
pub struct VersionQuery {
    /// Filter by path prefix
    pub path_prefix: Option<PathBuf>,
    
    /// Filter by timestamp range (start)
    pub since: Option<SystemTime>,
    
    /// Filter by timestamp range (end)
    pub until: Option<SystemTime>,
    
    /// Filter by operation types
    pub operation_types: Option<Vec<OperationType>>,
    
    /// Full-text search query
    pub text_query: Option<String>,
    
    /// Maximum number of results to return
    pub limit: Option<usize>,
    
    /// Number of results to skip
    pub offset: Option<usize>,
}

impl VersionQuery {
    /// Creates a new empty query
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Sets the path prefix filter
    pub fn with_path_prefix(mut self, prefix: impl AsRef<Path>) -> Self {
        self.path_prefix = Some(prefix.as_ref().to_path_buf());
        self
    }
    
    /// Sets the since timestamp filter
    pub fn with_since(mut self, since: SystemTime) -> Self {
        self.since = Some(since);
        self
    }
    
    /// Sets the until timestamp filter
    pub fn with_until(mut self, until: SystemTime) -> Self {
        self.until = Some(until);
        self
    }
    
    /// Sets the operation types filter
    pub fn with_operation_types(mut self, types: Vec<OperationType>) -> Self {
        self.operation_types = Some(types);
        self
    }
    
    /// Sets the text search query
    pub fn with_text_query(mut self, query: impl Into<String>) -> Self {
        self.text_query = Some(query.into());
        self
    }
    
    /// Sets the result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    
    /// Sets the result offset
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    
    /// Returns true if this query has any filters
    pub fn has_filters(&self) -> bool {
        self.path_prefix.is_some() || 
        self.since.is_some() || 
        self.until.is_some() || 
        self.operation_types.is_some() ||
        self.text_query.is_some()
    }
}