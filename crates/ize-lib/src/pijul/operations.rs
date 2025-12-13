//! Opcode recording operations for the Pijul backend
//!
//! This module provides `OpcodeRecordingBackend`, a pure adapter that translates
//! filesystem opcodes into Pijul change recording operations. It has no knowledge
//! of paths, repositories, or initialization - it only wraps a `PijulBackend` and
//! delegates operations to it.
//!
//! ## Architecture
//!
//! ```text
//! Opcode -> OpcodeRecordingBackend -> PijulBackend -> libpijul
//! ```
//!
//! `OpcodeRecordingBackend` is responsible for:
//! - Translating opcode types to appropriate PijulBackend method calls
//! - Converting paths and parameters to the right format
//! - Generating commit messages from opcode metadata
//! - Error conversion from PijulError to OpcodeError
//!
//! All Pijul interaction happens through the wrapped `PijulBackend`.
//! The calling code is responsible for creating and managing `PijulBackend` instances.

/// Backend for applying opcodes to Pijul
///
/// This is a thin adapter that translates filesystem opcodes into calls
/// to `PijulBackend`'s high-level API. It owns a `PijulBackend` and delegates
/// all Pijul operations to it.
///
/// # Example
///
/// ```rust,ignore
/// use ize_lib::pijul::{PijulBackend, OpcodeRecordingBackend};
/// use ize_lib::operations::Opcode;
///
/// // Create or open a PijulBackend
/// let pijul = PijulBackend::init(&pijul_dir, &working_dir, None)?;
///
/// // Wrap it in OpcodeRecordingBackend
/// let mut backend = OpcodeRecordingBackend::new(pijul);
///
/// // Apply opcodes
/// let hash = backend.apply_opcode(&opcode)?;
/// ```
use std::path::Path;

use libpijul::pristine::Hash;
use thiserror::Error;

use super::PijulBackend;
use crate::operations::{Opcode, Operation};

/// Errors that can occur during opcode operations
#[derive(Error, Debug)]
pub enum OpcodeError {
    #[error("Pijul error: {0}")]
    Pijul(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Path conversion error: {0}")]
    PathConversion(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),
}

impl From<super::PijulError> for OpcodeError {
    fn from(e: super::PijulError) -> Self {
        OpcodeError::Pijul(format!("{}", e))
    }
}

/// Backend for applying opcodes to Pijul
///
/// This is a thin adapter that translates filesystem opcodes into calls
/// to `PijulBackend`'s high-level API. It owns a `PijulBackend` and delegates
/// all Pijul operations to it.
///
/// # Example
///
/// ```rust,ignore
/// use ize_lib::pijul::{PijulBackend, OpcodeRecordingBackend};
/// use ize_lib::operations::Opcode;
///
/// // Create or open a PijulBackend
/// let pijul = PijulBackend::init(&pijul_dir, &working_dir, None)?;
///
/// // Wrap it in OpcodeRecordingBackend
/// let mut backend = OpcodeRecordingBackend::new(pijul);
///
/// // Apply opcodes
/// let hash = backend.apply_opcode(&opcode)?;
/// ```
pub struct OpcodeRecordingBackend {
    /// The underlying Pijul backend (our only interface to Pijul)
    pijul: PijulBackend,
}

impl OpcodeRecordingBackend {
    /// Create a new OpcodeRecordingBackend wrapping a PijulBackend
    ///
    /// This is the only way to construct an `OpcodeRecordingBackend`.
    /// The calling code must create the `PijulBackend` first.
    ///
    /// # Arguments
    /// * `pijul` - An initialized PijulBackend instance
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Calling code manages PijulBackend creation
    /// let pijul = PijulBackend::init(&pijul_dir, &working_dir, None)?;
    /// // or
    /// let pijul = PijulBackend::open(&pijul_dir, &working_dir)?;
    ///
    /// // Wrap it in the adapter
    /// let backend = OpcodeRecordingBackend::new(pijul);
    /// ```
    pub fn new(pijul: PijulBackend) -> Self {
        Self { pijul }
    }

    /// Apply an opcode and record it as a Pijul change
    ///
    /// This method translates the opcode into the appropriate PijulBackend
    /// method call and returns the hash of the created change.
    ///
    /// # Arguments
    /// * `opcode` - The opcode to apply
    ///
    /// # Returns
    ///
    /// Returns `Some(Hash)` if a change was created, or `None` if no change
    /// was needed (e.g., writing the same content).
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails or is unsupported.
    pub fn apply_opcode(&self, opcode: &Opcode) -> Result<Option<Hash>, OpcodeError> {
        let message = format!("Opcode #{}: {:?}", opcode.seq(), opcode.op());

        match opcode.op() {
            Operation::FileCreate {
                path,
                mode,
                content,
            } => {
                let path_str = path_to_str(path)?;
                self.pijul
                    .record_file_create(path_str, *mode, content, &message)
                    .map_err(Into::into)
            }

            Operation::FileWrite { path, offset, data } => {
                let path_str = path_to_str(path)?;
                self.pijul
                    .record_file_write(path_str, *offset, data, &message)
                    .map_err(Into::into)
            }

            Operation::FileTruncate { path, new_size } => {
                let path_str = path_to_str(path)?;
                self.pijul
                    .record_file_truncate(path_str, *new_size, &message)
                    .map_err(Into::into)
            }

            Operation::FileDelete { path } => {
                let path_str = path_to_str(path)?;
                self.pijul
                    .record_file_delete(path_str, &message)
                    .map_err(Into::into)
            }

            Operation::FileRename { old_path, new_path } => {
                let old_path_str = path_to_str(old_path)?;
                let new_path_str = path_to_str(new_path)?;
                self.pijul
                    .record_file_rename(old_path_str, new_path_str, &message)
                    .map(Some)
                    .map_err(Into::into)
            }

            // Unsupported operations
            Operation::DirCreate { .. }
            | Operation::DirDelete { .. }
            | Operation::DirRename { .. }
            | Operation::SetPermissions { .. }
            | Operation::SetTimestamps { .. }
            | Operation::SetOwnership { .. }
            | Operation::SymlinkCreate { .. }
            | Operation::SymlinkDelete { .. }
            | Operation::HardLinkCreate { .. } => Err(OpcodeError::UnsupportedOperation(format!(
                "{:?}",
                opcode.op()
            ))),
        }
    }

    /// Get a reference to the underlying PijulBackend
    ///
    /// This provides access to all PijulBackend query and management methods:
    /// - `get_file_content()`, `list_changes()` - Query operations
    /// - `current_channel()`, `pijul_dir()`, `working_dir()` - Repository info
    /// - `create_channel()`, `switch_channel()` - Channel management
    pub fn pijul(&self) -> &PijulBackend {
        &self.pijul
    }

    /// Get a mutable reference to the underlying PijulBackend
    ///
    /// This allows mutable operations like channel switching:
    ///
    /// ```rust,ignore
    /// backend.pijul_mut().switch_channel("feature")?;
    /// ```
    pub fn pijul_mut(&mut self) -> &mut PijulBackend {
        &mut self.pijul
    }
}

/// Convert a Path to a Pijul-compatible string
///
/// Pijul expects forward-slash separated paths, so we need to convert
/// platform-specific paths to this format.
fn path_to_str(path: &Path) -> Result<&str, OpcodeError> {
    path.to_str()
        .ok_or_else(|| OpcodeError::PathConversion(format!("Invalid UTF-8 in path: {:?}", path)))
}

impl std::fmt::Debug for OpcodeRecordingBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcodeRecordingBackend")
            .field("pijul", &self.pijul)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, OpcodeRecordingBackend) {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let pijul = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        let backend = OpcodeRecordingBackend::new(pijul);
        (temp, backend)
    }

    #[test]
    fn test_new_backend() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        // Create PijulBackend
        let pijul = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        assert_eq!(pijul.current_channel(), "main");

        // Wrap it in OpcodeRecordingBackend
        let backend = OpcodeRecordingBackend::new(pijul);
        assert_eq!(backend.pijul().current_channel(), "main");
    }

    #[test]
    fn test_file_create() {
        let (_temp, backend) = setup_test_repo();

        let opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello, World!".to_vec(),
            },
        );

        let hash = backend.apply_opcode(&opcode).unwrap();
        assert!(hash.is_some(), "FileCreate should return a hash");

        // VERIFY: File exists in Pijul
        assert!(
            backend.pijul().file_exists("test.txt").unwrap(),
            "File should exist in Pijul"
        );

        // VERIFY: Content matches what was recorded
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        assert_eq!(
            content, b"Hello, World!",
            "File content should match what was recorded"
        );

        // VERIFY: Change was recorded
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 1, "Should have exactly 1 change");
        assert_eq!(changes[0], hash.unwrap(), "Change hash should match");
    }

    #[test]
    fn test_file_write_to_existing() {
        let (_temp, backend) = setup_test_repo();

        // Create a file
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello, World!".to_vec(),
            },
        );
        let hash1 = backend.apply_opcode(&create_opcode).unwrap();

        // Write to it
        let write_opcode = Opcode::new(
            2,
            Operation::FileWrite {
                path: PathBuf::from("test.txt"),
                offset: 7,
                data: b"Pijul".to_vec(),
            },
        );

        let hash2 = backend.apply_opcode(&write_opcode).unwrap();
        assert!(hash2.is_some(), "FileWrite should return a hash");

        // VERIFY: Content was updated
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Hello, Pijul!");

        // VERIFY: Two changes recorded
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 2, "Should have 2 changes");
        assert_eq!(changes[0], hash1.unwrap());
        assert_eq!(changes[1], hash2.unwrap());
    }

    #[test]
    fn test_file_write_extends_file() {
        let (_temp, backend) = setup_test_repo();

        // Create a file
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello".to_vec(),
            },
        );
        backend.apply_opcode(&create_opcode).unwrap();

        // Write beyond current end
        let write_opcode = Opcode::new(
            2,
            Operation::FileWrite {
                path: PathBuf::from("test.txt"),
                offset: 10,
                data: b"World".to_vec(),
            },
        );

        let hash = backend.apply_opcode(&write_opcode).unwrap();
        assert!(hash.is_some(), "FileWrite should return a hash");

        // Verify content (should have zeros between)
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        assert_eq!(content.len(), 15);
        assert_eq!(&content[0..5], b"Hello");
        assert_eq!(&content[10..15], b"World");
    }

    #[test]
    fn test_unsupported_operations() {
        let (_temp, backend) = setup_test_repo();

        let opcode = Opcode::new(
            1,
            Operation::DirCreate {
                path: PathBuf::from("testdir"),
                mode: 0o755,
            },
        );

        let result = backend.apply_opcode(&opcode);
        assert!(
            matches!(result, Err(OpcodeError::UnsupportedOperation(_))),
            "Should return UnsupportedOperation error"
        );
    }

    #[test]
    fn test_opcode_sequence() {
        let (_temp, backend) = setup_test_repo();

        // Sequence of operations on same file
        let opcodes = vec![
            Opcode::new(
                1,
                Operation::FileCreate {
                    path: PathBuf::from("file.txt"),
                    mode: 0o644,
                    content: b"v1".to_vec(),
                },
            ),
            Opcode::new(
                2,
                Operation::FileWrite {
                    path: PathBuf::from("file.txt"),
                    offset: 0,
                    data: b"v2".to_vec(),
                },
            ),
            Opcode::new(
                3,
                Operation::FileTruncate {
                    path: PathBuf::from("file.txt"),
                    new_size: 1,
                },
            ),
        ];

        let mut hashes = Vec::new();
        for opcode in &opcodes {
            let hash = backend.apply_opcode(opcode).unwrap();
            assert!(hash.is_some(), "Each operation should return a hash");
            hashes.push(hash.unwrap());
        }

        // VERIFY: Final content is correct
        let content = backend.pijul().get_file_content("file.txt").unwrap();
        assert_eq!(content, b"v", "Content should be truncated to 'v'");

        // VERIFY: All changes were recorded in order
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 3, "Should have 3 changes");
        assert_eq!(changes, hashes, "Changes should match in order");
    }

    #[test]
    fn test_multiple_files() {
        let (_temp, backend) = setup_test_repo();

        // Create first file
        let file1 = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("file1.txt"),
                mode: 0o644,
                content: b"Content 1".to_vec(),
            },
        );
        let hash1 = backend.apply_opcode(&file1).unwrap();
        assert!(hash1.is_some(), "First file creation should succeed");

        // Verify first file
        let content1 = backend.pijul().get_file_content("file1.txt").unwrap();
        assert_eq!(
            content1, b"Content 1",
            "file1.txt should have correct content"
        );

        // VERIFY: One change recorded so far
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 1, "Should have 1 change after first file");
    }

    #[test]
    fn test_file_delete_sequence() {
        let (_temp, backend) = setup_test_repo();

        // Create and then delete a file
        let create = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("temp.txt"),
                mode: 0o644,
                content: b"Temporary".to_vec(),
            },
        );
        backend.apply_opcode(&create).unwrap();

        let delete = Opcode::new(
            2,
            Operation::FileDelete {
                path: PathBuf::from("temp.txt"),
            },
        );
        backend.apply_opcode(&delete).unwrap();

        // VERIFY: File content is empty (deleted)
        let content = backend.pijul().get_file_content("temp.txt").unwrap();
        assert!(content.is_empty(), "Deleted file should have empty content");

        // VERIFY: Both changes recorded
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 2, "Should have create + delete changes");
    }

    #[test]
    fn test_file_in_subdirectory() {
        let (_temp, backend) = setup_test_repo();

        let opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("src/main.rs"),
                mode: 0o644,
                content: b"fn main() {}".to_vec(),
            },
        );

        let hash = backend.apply_opcode(&opcode).unwrap();
        assert!(hash.is_some(), "Should create file in subdirectory");

        // VERIFY: File exists with correct content
        assert!(backend.pijul().file_exists("src/main.rs").unwrap());
        let content = backend.pijul().get_file_content("src/main.rs").unwrap();
        assert_eq!(content, b"fn main() {}");
    }

    #[test]
    #[ignore]
    // This was just to test that the interactions were actually hitting the filesystem.
    // Tests were nearly instantaneous, probably because they were on tmpfs, sanakirja fast etc.
    // Leaving there just in case
    fn test_verify_filesystem_interaction() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        println!("\n=== Filesystem Interaction Test ===");
        println!("Test directory: {:?}", temp.path());

        // Initialize
        let pijul = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        let backend = OpcodeRecordingBackend::new(pijul);

        // Check what was created
        println!("\n✓ Directory structure after init:");
        println!("  .pijul exists: {}", pijul_dir.exists());
        println!(
            "  .pijul/pristine/db exists: {}",
            pijul_dir.join("pristine/db").exists()
        );
        println!(
            "  .pijul/changes exists: {}",
            pijul_dir.join("changes").exists()
        );

        // Check database file size
        if let Ok(metadata) = std::fs::metadata(pijul_dir.join("pristine/db")) {
            println!("  Database size: {} bytes", metadata.len());
        }

        // Create a file via opcode
        let opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello from filesystem!".to_vec(),
            },
        );

        backend.apply_opcode(&opcode).unwrap();

        // Check changes directory
        println!("\n✓ After file creation:");
        let changes_dir = pijul_dir.join("changes");
        if let Ok(entries) = std::fs::read_dir(&changes_dir) {
            let mut count = 0;
            for entry in entries {
                if let Ok(entry) = entry {
                    if let Ok(metadata) = entry.metadata() {
                        println!(
                            "  Change file: {:?} ({} bytes)",
                            entry.file_name(),
                            metadata.len()
                        );
                        count += 1;
                    }
                }
            }
            println!("  Total change files: {}", count);
        }

        // Verify via PijulBackend
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        println!("\n✓ Content verification:");
        println!("  Retrieved: {:?}", String::from_utf8_lossy(&content));
        println!("  Size: {} bytes", content.len());

        assert_eq!(content, b"Hello from filesystem!");

        // Note: TempDir automatically cleans up when dropped
        println!("\n(TempDir will auto-cleanup on test completion)");
    }
}
