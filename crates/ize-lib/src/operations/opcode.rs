//! Opcode types for filesystem mutation tracking
//!
//! This module defines the core types for capturing filesystem operations:
//! - [`Opcode`]: A single filesystem operation with metadata
//! - [`Operation`]: The specific operation type and its data
//!
//! # Design Principles
//!
//! 1. **Self-contained**: Each opcode has everything needed to replay it
//! 2. **Immutable**: Opcodes are append-only, never modified
//! 3. **Ordered**: Monotonic sequence numbers for ordering
//! 4. **Path-based**: Uses paths, not inodes (inodes are ephemeral)
//!
//! # Example
//!
//! ```
//! use std::path::PathBuf;
//! use ize_lib::operations::{Opcode, Operation};
//!
//! let opcode = Opcode::new(
//!     1,  // sequence number
//!     Operation::FileCreate {
//!         path: PathBuf::from("hello.txt"),
//!         mode: 0o644,
//!         content: b"Hello, world!".to_vec(),
//!     },
//! );
//!
//! assert_eq!(opcode.seq(), 1);
//! assert!(matches!(opcode.op(), Operation::FileCreate { .. }));
//! ```

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single filesystem operation with all necessary context.
///
/// Opcodes are the fundamental unit of change tracking in Ize. Each opcode
/// represents a single mutation to the filesystem and contains everything
/// needed to understand and replay the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opcode {
    /// Monotonic sequence number for ordering.
    ///
    /// Operations on the same path must be applied in sequence order.
    /// Operations on different paths are commutative.
    seq: u64,

    /// When the operation occurred (Unix timestamp in nanoseconds).
    timestamp: u64,

    /// The operation itself.
    op: Operation,
}

impl Opcode {
    /// Create a new opcode with the current timestamp.
    ///
    /// # Arguments
    /// * `seq` - Monotonic sequence number
    /// * `op` - The operation to record
    pub fn new(seq: u64, op: Operation) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        Self { seq, timestamp, op }
    }

    /// Create a new opcode with a specific timestamp.
    ///
    /// Useful for testing or replaying operations.
    pub fn with_timestamp(seq: u64, timestamp: u64, op: Operation) -> Self {
        Self { seq, timestamp, op }
    }

    /// Get the sequence number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Get the timestamp (nanoseconds since Unix epoch).
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Get a reference to the operation.
    pub fn op(&self) -> &Operation {
        &self.op
    }

    /// Consume self and return the operation.
    pub fn into_op(self) -> Operation {
        self.op
    }

    /// Get the primary path affected by this operation.
    ///
    /// For rename operations, returns the source (old) path.
    pub fn path(&self) -> &PathBuf {
        self.op.path()
    }

    /// Check if this operation affects the given path.
    ///
    /// For rename operations, checks both source and destination.
    pub fn affects_path(&self, path: &PathBuf) -> bool {
        self.op.affects_path(path)
    }
}

/// The specific operation type and its data.
///
/// Each variant captures all data needed to replay the operation.
/// Paths are always relative to the working directory root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    // =========================================================================
    // File Operations
    // =========================================================================
    /// A new file is created.
    ///
    /// Captured on `create()` FUSE call or first write to a new file.
    FileCreate {
        /// Full relative path from working root
        path: PathBuf,
        /// Unix permissions (e.g., 0o644)
        mode: u32,
        /// Initial content (may be empty if writes follow)
        content: Vec<u8>,
    },

    /// Data is written to an existing file.
    ///
    /// Captured on `write()` FUSE call.
    FileWrite {
        /// Full relative path
        path: PathBuf,
        /// Byte offset where write begins
        offset: u64,
        /// Bytes written
        data: Vec<u8>,
    },

    /// File size is changed.
    ///
    /// Captured on `setattr()` with size change or `truncate()` syscall.
    FileTruncate {
        /// Full relative path
        path: PathBuf,
        /// New file size in bytes
        new_size: u64,
    },

    /// A file is removed (unlinked).
    ///
    /// Captured on `unlink()` FUSE call for regular files.
    FileDelete {
        /// Full relative path (at time of deletion)
        path: PathBuf,
    },

    /// A file is moved or renamed.
    ///
    /// Captured on `rename()` FUSE call where source is a file.
    FileRename {
        /// Original path
        old_path: PathBuf,
        /// New path
        new_path: PathBuf,
    },

    // =========================================================================
    // Directory Operations
    // =========================================================================
    /// A new directory is created.
    ///
    /// Captured on `mkdir()` FUSE call.
    DirCreate {
        /// Full relative path
        path: PathBuf,
        /// Unix permissions (e.g., 0o755)
        mode: u32,
    },

    /// An empty directory is removed.
    ///
    /// Captured on `rmdir()` FUSE call.
    DirDelete {
        /// Full relative path
        path: PathBuf,
    },

    /// A directory is moved or renamed.
    ///
    /// Captured on `rename()` FUSE call where source is a directory.
    /// All contents move with the directory.
    DirRename {
        /// Original path
        old_path: PathBuf,
        /// New path
        new_path: PathBuf,
    },

    // =========================================================================
    // Metadata Operations
    // =========================================================================
    /// File or directory permissions are changed.
    ///
    /// Captured on `setattr()` with mode change or `chmod()` syscall.
    SetPermissions {
        /// Full relative path
        path: PathBuf,
        /// New permission bits (lower 12 bits of mode)
        mode: u32,
    },

    /// File or directory timestamps are explicitly modified.
    ///
    /// Captured on `setattr()` with time changes or `utimes()`/`touch`.
    /// Only captures explicit changes, not implicit updates from writes.
    SetTimestamps {
        /// Full relative path
        path: PathBuf,
        /// Access time (Unix timestamp in seconds, if changed)
        atime: Option<u64>,
        /// Modification time (Unix timestamp in seconds, if changed)
        mtime: Option<u64>,
    },

    /// File or directory ownership is changed.
    ///
    /// Captured on `setattr()` with uid/gid changes or `chown()` syscall.
    SetOwnership {
        /// Full relative path
        path: PathBuf,
        /// New owner UID (if changed)
        uid: Option<u32>,
        /// New group GID (if changed)
        gid: Option<u32>,
    },

    // =========================================================================
    // Symbolic Link Operations
    // =========================================================================
    /// A symbolic link is created.
    ///
    /// Captured on `symlink()` FUSE call.
    SymlinkCreate {
        /// Path of the symlink itself
        path: PathBuf,
        /// What the symlink points to (may be relative or absolute)
        target: PathBuf,
    },

    /// A symbolic link is removed.
    ///
    /// Captured on `unlink()` FUSE call where target is a symlink.
    SymlinkDelete {
        /// Path of the symlink
        path: PathBuf,
    },

    // =========================================================================
    // Hard Link Operations
    // =========================================================================
    /// A hard link is created.
    ///
    /// Captured on `link()` FUSE call.
    HardLinkCreate {
        /// Existing file to link to
        existing_path: PathBuf,
        /// New link path
        new_path: PathBuf,
    },
}

impl Operation {
    /// Get the primary path affected by this operation.
    ///
    /// For rename operations, returns the source (old) path.
    /// For hard link creation, returns the new link path.
    pub fn path(&self) -> &PathBuf {
        match self {
            // File operations
            Operation::FileCreate { path, .. } => path,
            Operation::FileWrite { path, .. } => path,
            Operation::FileTruncate { path, .. } => path,
            Operation::FileDelete { path } => path,
            Operation::FileRename { old_path, .. } => old_path,

            // Directory operations
            Operation::DirCreate { path, .. } => path,
            Operation::DirDelete { path } => path,
            Operation::DirRename { old_path, .. } => old_path,

            // Metadata operations
            Operation::SetPermissions { path, .. } => path,
            Operation::SetTimestamps { path, .. } => path,
            Operation::SetOwnership { path, .. } => path,

            // Symlink operations
            Operation::SymlinkCreate { path, .. } => path,
            Operation::SymlinkDelete { path } => path,

            // Hard link operations
            Operation::HardLinkCreate { new_path, .. } => new_path,
        }
    }

    /// Check if this operation affects the given path.
    ///
    /// For rename operations, checks both source and destination.
    /// For hard link creation, checks both existing and new paths.
    pub fn affects_path(&self, path: &PathBuf) -> bool {
        match self {
            Operation::FileRename { old_path, new_path } => old_path == path || new_path == path,
            Operation::DirRename { old_path, new_path } => old_path == path || new_path == path,
            Operation::HardLinkCreate {
                existing_path,
                new_path,
            } => existing_path == path || new_path == path,
            _ => self.path() == path,
        }
    }

    /// Check if this is a file operation (not directory, metadata, or link).
    pub fn is_file_op(&self) -> bool {
        matches!(
            self,
            Operation::FileCreate { .. }
                | Operation::FileWrite { .. }
                | Operation::FileTruncate { .. }
                | Operation::FileDelete { .. }
                | Operation::FileRename { .. }
        )
    }

    /// Check if this is a directory operation.
    pub fn is_dir_op(&self) -> bool {
        matches!(
            self,
            Operation::DirCreate { .. } | Operation::DirDelete { .. } | Operation::DirRename { .. }
        )
    }

    /// Check if this is a metadata operation.
    pub fn is_metadata_op(&self) -> bool {
        matches!(
            self,
            Operation::SetPermissions { .. }
                | Operation::SetTimestamps { .. }
                | Operation::SetOwnership { .. }
        )
    }

    /// Check if this is a link operation (symlink or hard link).
    pub fn is_link_op(&self) -> bool {
        matches!(
            self,
            Operation::SymlinkCreate { .. }
                | Operation::SymlinkDelete { .. }
                | Operation::HardLinkCreate { .. }
        )
    }

    /// Check if this operation modifies file content.
    pub fn modifies_content(&self) -> bool {
        matches!(
            self,
            Operation::FileCreate { .. }
                | Operation::FileWrite { .. }
                | Operation::FileTruncate { .. }
        )
    }

    /// Check if this is a destructive operation (delete or overwriting rename).
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            Operation::FileDelete { .. }
                | Operation::DirDelete { .. }
                | Operation::SymlinkDelete { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opcode_creation() {
        let op = Operation::FileCreate {
            path: PathBuf::from("test.txt"),
            mode: 0o644,
            content: b"hello".to_vec(),
        };
        let opcode = Opcode::new(1, op);

        assert_eq!(opcode.seq(), 1);
        assert!(opcode.timestamp() > 0);
        assert!(matches!(opcode.op(), Operation::FileCreate { .. }));
    }

    #[test]
    fn test_opcode_with_timestamp() {
        let op = Operation::FileDelete {
            path: PathBuf::from("delete.txt"),
        };
        let opcode = Opcode::with_timestamp(42, 1234567890, op);

        assert_eq!(opcode.seq(), 42);
        assert_eq!(opcode.timestamp(), 1234567890);
    }

    #[test]
    fn test_opcode_path() {
        let op = Operation::FileWrite {
            path: PathBuf::from("data.bin"),
            offset: 100,
            data: vec![1, 2, 3],
        };
        let opcode = Opcode::new(1, op);

        assert_eq!(opcode.path(), &PathBuf::from("data.bin"));
    }

    #[test]
    fn test_operation_path_for_rename() {
        let op = Operation::FileRename {
            old_path: PathBuf::from("old.txt"),
            new_path: PathBuf::from("new.txt"),
        };

        assert_eq!(op.path(), &PathBuf::from("old.txt"));
        assert!(op.affects_path(&PathBuf::from("old.txt")));
        assert!(op.affects_path(&PathBuf::from("new.txt")));
        assert!(!op.affects_path(&PathBuf::from("other.txt")));
    }

    #[test]
    fn test_operation_type_checks() {
        let file_op = Operation::FileCreate {
            path: PathBuf::from("f.txt"),
            mode: 0o644,
            content: vec![],
        };
        assert!(file_op.is_file_op());
        assert!(!file_op.is_dir_op());
        assert!(!file_op.is_metadata_op());
        assert!(!file_op.is_link_op());

        let dir_op = Operation::DirCreate {
            path: PathBuf::from("dir"),
            mode: 0o755,
        };
        assert!(!dir_op.is_file_op());
        assert!(dir_op.is_dir_op());

        let meta_op = Operation::SetPermissions {
            path: PathBuf::from("f.txt"),
            mode: 0o600,
        };
        assert!(meta_op.is_metadata_op());

        let link_op = Operation::SymlinkCreate {
            path: PathBuf::from("link"),
            target: PathBuf::from("target"),
        };
        assert!(link_op.is_link_op());
    }

    #[test]
    fn test_operation_modifies_content() {
        assert!(Operation::FileCreate {
            path: PathBuf::from("f"),
            mode: 0,
            content: vec![]
        }
        .modifies_content());

        assert!(Operation::FileWrite {
            path: PathBuf::from("f"),
            offset: 0,
            data: vec![]
        }
        .modifies_content());

        assert!(Operation::FileTruncate {
            path: PathBuf::from("f"),
            new_size: 0
        }
        .modifies_content());

        assert!(!Operation::FileDelete {
            path: PathBuf::from("f")
        }
        .modifies_content());

        assert!(!Operation::SetPermissions {
            path: PathBuf::from("f"),
            mode: 0
        }
        .modifies_content());
    }

    #[test]
    fn test_operation_is_destructive() {
        assert!(Operation::FileDelete {
            path: PathBuf::from("f")
        }
        .is_destructive());

        assert!(Operation::DirDelete {
            path: PathBuf::from("d")
        }
        .is_destructive());

        assert!(Operation::SymlinkDelete {
            path: PathBuf::from("l")
        }
        .is_destructive());

        assert!(!Operation::FileCreate {
            path: PathBuf::from("f"),
            mode: 0,
            content: vec![]
        }
        .is_destructive());
    }

    #[test]
    fn test_all_operation_variants() {
        // Ensure all variants can be created and have valid paths
        let ops = vec![
            Operation::FileCreate {
                path: PathBuf::from("a"),
                mode: 0o644,
                content: vec![],
            },
            Operation::FileWrite {
                path: PathBuf::from("b"),
                offset: 0,
                data: vec![],
            },
            Operation::FileTruncate {
                path: PathBuf::from("c"),
                new_size: 0,
            },
            Operation::FileDelete {
                path: PathBuf::from("d"),
            },
            Operation::FileRename {
                old_path: PathBuf::from("e"),
                new_path: PathBuf::from("f"),
            },
            Operation::DirCreate {
                path: PathBuf::from("g"),
                mode: 0o755,
            },
            Operation::DirDelete {
                path: PathBuf::from("h"),
            },
            Operation::DirRename {
                old_path: PathBuf::from("i"),
                new_path: PathBuf::from("j"),
            },
            Operation::SetPermissions {
                path: PathBuf::from("k"),
                mode: 0o600,
            },
            Operation::SetTimestamps {
                path: PathBuf::from("l"),
                atime: Some(1000),
                mtime: Some(2000),
            },
            Operation::SetOwnership {
                path: PathBuf::from("m"),
                uid: Some(1000),
                gid: Some(1000),
            },
            Operation::SymlinkCreate {
                path: PathBuf::from("n"),
                target: PathBuf::from("target"),
            },
            Operation::SymlinkDelete {
                path: PathBuf::from("o"),
            },
            Operation::HardLinkCreate {
                existing_path: PathBuf::from("p"),
                new_path: PathBuf::from("q"),
            },
        ];

        for op in ops {
            // Verify path() doesn't panic
            let _ = op.path();
            // Verify affects_path works
            assert!(op.affects_path(op.path()));
        }
    }

    #[test]
    fn test_opcode_into_op() {
        let op = Operation::FileDelete {
            path: PathBuf::from("test.txt"),
        };
        let opcode = Opcode::new(1, op.clone());

        let extracted = opcode.into_op();
        assert_eq!(extracted, op);
    }
}
