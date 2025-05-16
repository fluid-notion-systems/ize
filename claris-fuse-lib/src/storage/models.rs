use std::time::SystemTime;

/// The type of operation that was performed on a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    /// The file was created
    Create,
    /// The file was written to
    Write,
    /// The file was renamed
    Rename,
    /// The file was deleted
    Delete,
}

/// Represents a version of a file in storage
#[derive(Debug, Clone)]
pub struct FileVersion {
    /// Unique identifier for this version
    pub id: u64,
    /// Path to the file at the time this version was created
    pub path: String,
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
}