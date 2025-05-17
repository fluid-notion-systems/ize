use std::convert::TryFrom;
use std::fmt;
use thiserror::Error;

/// Errors specific to filesystem operations
#[derive(Error, Debug)]
pub enum FilesystemError {
    /// Error when converting between operation types
    #[error("Invalid operation type: {0}")]
    InvalidOperationType(String),
}

/// General operation type that encompasses all filesystem operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationType {
    /// File-specific operations
    File(FileOperationType),
    /// Directory-specific operations
    Directory(DirectoryOperationType),
    /// Metadata operations that can apply to both files and directories
    Metadata(MetadataOperationType),
    /// Link operations
    Link(LinkOperationType),
}

/// Operations specific to files
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileOperationType {
    /// Create a new file
    Create,
    /// Write to an existing file
    Write,
    /// Delete a file
    Delete,
    /// Truncate a file to a specific size
    Truncate,
}

/// Operations specific to directories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectoryOperationType {
    /// Create a new directory
    Create,
    /// Delete a directory
    Delete,
}

/// Operations that modify metadata and can apply to both files and directories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataOperationType {
    /// Change file permissions
    Chmod,
    /// Change file ownership
    Chown,
    /// Change file times
    Utimens,
    /// Set extended attributes
    SetXattr,
    /// Remove extended attributes
    RemoveXattr,
    /// Rename a file or directory
    Rename,
}

/// Operations related to links
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkOperationType {
    /// Create a symbolic link
    Symlink,
    /// Create a hard link
    Hardlink,
}

impl OperationType {
    /// Returns true if this operation type typically has associated content
    pub fn has_content(&self) -> bool {
        matches!(
            self,
            OperationType::File(FileOperationType::Create)
                | OperationType::File(FileOperationType::Write)
                | OperationType::File(FileOperationType::Truncate)
        )
    }

    /// Returns true if this operation type changes file metadata
    pub fn changes_metadata(&self) -> bool {
        matches!(
            self,
            OperationType::Metadata(_)
        )
    }

    /// Returns the string representation of this operation type
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationType::File(FileOperationType::Create) => "FileCreate",
            OperationType::File(FileOperationType::Write) => "FileWrite",
            OperationType::File(FileOperationType::Delete) => "FileDelete",
            OperationType::File(FileOperationType::Truncate) => "FileTruncate",
            OperationType::Directory(DirectoryOperationType::Create) => "DirCreate",
            OperationType::Directory(DirectoryOperationType::Delete) => "DirDelete",
            OperationType::Metadata(MetadataOperationType::Chmod) => "Chmod",
            OperationType::Metadata(MetadataOperationType::Chown) => "Chown",
            OperationType::Metadata(MetadataOperationType::Utimens) => "Utimens",
            OperationType::Metadata(MetadataOperationType::SetXattr) => "SetXattr",
            OperationType::Metadata(MetadataOperationType::RemoveXattr) => "RemoveXattr",
            OperationType::Metadata(MetadataOperationType::Rename) => "Rename",
            OperationType::Link(LinkOperationType::Symlink) => "Symlink",
            OperationType::Link(LinkOperationType::Hardlink) => "Hardlink",
        }
    }
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl TryFrom<&str> for OperationType {
    type Error = FilesystemError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "FileCreate" => Ok(OperationType::File(FileOperationType::Create)),
            "FileWrite" => Ok(OperationType::File(FileOperationType::Write)),
            "FileDelete" => Ok(OperationType::File(FileOperationType::Delete)),
            "FileTruncate" => Ok(OperationType::File(FileOperationType::Truncate)),
            "DirCreate" => Ok(OperationType::Directory(DirectoryOperationType::Create)),
            "DirDelete" => Ok(OperationType::Directory(DirectoryOperationType::Delete)),
            "Chmod" => Ok(OperationType::Metadata(MetadataOperationType::Chmod)),
            "Chown" => Ok(OperationType::Metadata(MetadataOperationType::Chown)),
            "Utimens" => Ok(OperationType::Metadata(MetadataOperationType::Utimens)),
            "SetXattr" => Ok(OperationType::Metadata(MetadataOperationType::SetXattr)),
            "RemoveXattr" => Ok(OperationType::Metadata(MetadataOperationType::RemoveXattr)),
            "Rename" => Ok(OperationType::Metadata(MetadataOperationType::Rename)),
            "Symlink" => Ok(OperationType::Link(LinkOperationType::Symlink)),
            "Hardlink" => Ok(OperationType::Link(LinkOperationType::Hardlink)),
            _ => Err(FilesystemError::InvalidOperationType(s.to_string())),
        }
    }
}

// For backward compatibility with the legacy OperationType
impl From<crate::domain::models::OperationType> for OperationType {
    fn from(op: crate::domain::models::OperationType) -> Self {
        use crate::domain::models::OperationType as LegacyOp;
        
        match op {
            LegacyOp::Create => OperationType::File(FileOperationType::Create),
            LegacyOp::Write => OperationType::File(FileOperationType::Write),
            LegacyOp::Delete => OperationType::File(FileOperationType::Delete),
            LegacyOp::Truncate => OperationType::File(FileOperationType::Truncate),
            LegacyOp::Rename => OperationType::Metadata(MetadataOperationType::Rename),
            LegacyOp::Mkdir => OperationType::Directory(DirectoryOperationType::Create),
            LegacyOp::Rmdir => OperationType::Directory(DirectoryOperationType::Delete),
            LegacyOp::Symlink => OperationType::Link(LinkOperationType::Symlink),
            LegacyOp::Link => OperationType::Link(LinkOperationType::Hardlink),
            LegacyOp::Chmod => OperationType::Metadata(MetadataOperationType::Chmod),
            LegacyOp::Chown => OperationType::Metadata(MetadataOperationType::Chown),
            LegacyOp::Utimens => OperationType::Metadata(MetadataOperationType::Utimens),
            LegacyOp::SetXattr => OperationType::Metadata(MetadataOperationType::SetXattr),
            LegacyOp::RemoveXattr => OperationType::Metadata(MetadataOperationType::RemoveXattr),
        }
    }
}

// For backward compatibility with the legacy OperationType
impl From<OperationType> for crate::domain::models::OperationType {
    fn from(op: OperationType) -> Self {
        use crate::domain::models::OperationType as LegacyOp;
        
        match op {
            OperationType::File(FileOperationType::Create) => LegacyOp::Create,
            OperationType::File(FileOperationType::Write) => LegacyOp::Write,
            OperationType::File(FileOperationType::Delete) => LegacyOp::Delete,
            OperationType::File(FileOperationType::Truncate) => LegacyOp::Truncate,
            OperationType::Directory(DirectoryOperationType::Create) => LegacyOp::Mkdir,
            OperationType::Directory(DirectoryOperationType::Delete) => LegacyOp::Rmdir,
            OperationType::Metadata(MetadataOperationType::Chmod) => LegacyOp::Chmod,
            OperationType::Metadata(MetadataOperationType::Chown) => LegacyOp::Chown,
            OperationType::Metadata(MetadataOperationType::Utimens) => LegacyOp::Utimens,
            OperationType::Metadata(MetadataOperationType::SetXattr) => LegacyOp::SetXattr,
            OperationType::Metadata(MetadataOperationType::RemoveXattr) => LegacyOp::RemoveXattr,
            OperationType::Metadata(MetadataOperationType::Rename) => LegacyOp::Rename,
            OperationType::Link(LinkOperationType::Symlink) => LegacyOp::Symlink,
            OperationType::Link(LinkOperationType::Hardlink) => LegacyOp::Link,
        }
    }
}