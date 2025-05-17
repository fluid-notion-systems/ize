use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur in filesystem operations
#[derive(Error, Debug)]
pub enum FsError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    #[error("Path not found: {0}")]
    PathNotFound(PathBuf),

    #[error("Permission denied for path: {0}")]
    PermissionDenied(PathBuf),

    #[error("Path already exists: {0}")]
    PathExists(PathBuf),

    #[error("Invalid file type for path: {0}")]
    InvalidFileType(PathBuf),

    #[error("Invalid operation in read-only mode")]
    ReadOnlyFs,

    #[error("Database file cannot be inside mount point")]
    DbInsideMountPoint,

    #[error("Failed to allocate inode for path: {0}")]
    InodeAllocationFailed(PathBuf),

    #[error("Inode {0} not found")]
    InodeNotFound(u64),

    #[error("Failed to convert path: {0}")]
    PathConversionError(PathBuf),

    #[error("Operation not supported: {0}")]
    OperationNotSupported(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Extension trait for converting io::Error into more specific FsError types
pub trait IoErrorExt {
    /// Convert an io::Error into a more specific FsError based on the error kind and path
    fn into_fs_error(self, path: impl AsRef<Path>) -> FsError;
}

impl IoErrorExt for io::Error {
    fn into_fs_error(self, path: impl AsRef<Path>) -> FsError {
        let path_buf = path.as_ref().to_path_buf();

        match self.kind() {
            io::ErrorKind::NotFound => FsError::PathNotFound(path_buf),
            io::ErrorKind::PermissionDenied => FsError::PermissionDenied(path_buf),
            io::ErrorKind::AlreadyExists => FsError::PathExists(path_buf),
            _ => FsError::Io(self),
        }
    }
}

/// Extension trait to convert FsError to i32 error codes for FUSE
pub trait FsErrorCode {
    /// Convert to a libc error code for FUSE replies
    fn to_error_code(&self) -> i32;
}

impl FsErrorCode for FsError {
    fn to_error_code(&self) -> i32 {
        use libc::*;

        match self {
            FsError::Io(e) => e.raw_os_error().unwrap_or(EIO),
            FsError::InvalidPath(_) => EINVAL,
            FsError::PathNotFound(_) => ENOENT,
            FsError::PermissionDenied(_) => EACCES,
            FsError::PathExists(_) => EEXIST,
            FsError::InvalidFileType(_) => EINVAL,
            FsError::ReadOnlyFs => EROFS,
            FsError::DbInsideMountPoint => EINVAL,
            FsError::InodeAllocationFailed(_) => ENOMEM,
            FsError::InodeNotFound(_) => ENOENT,
            FsError::PathConversionError(_) => EINVAL,
            FsError::OperationNotSupported(_) => ENOSYS,
            FsError::Internal(_) => EIO,
        }
    }
}

/// Result type for filesystem operations
pub type FsResult<T> = Result<T, FsError>;
