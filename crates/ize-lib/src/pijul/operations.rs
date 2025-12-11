//! Opcode recording operations for the Pijul backend
//!
//! This module handles converting filesystem opcodes into Pijul changes.
//! The key flow is:
//! 1. Read current file content from pristine (via Graph retrieval)
//! 2. Apply the operation in memory to get new bytes
//! 3. Diff old Graph vs new bytes directly using libpijul's diff
//! 4. Create a Change from that diff
//! 5. Apply the Change to the pristine
//!
//! This approach bypasses the working copy abstraction entirely, diffing
//! directly from old content to new content without needing to maintain
//! a full in-memory working copy.

use std::path::Path;

use libpijul::alive_retrieve;
use libpijul::change::{ChangeError, ChangeHeader};
use libpijul::changestore::filesystem::Error as ChangeStoreError;
use libpijul::changestore::filesystem::FileSystem as ChangeStore;
use libpijul::changestore::ChangeStore as ChangeStoreTrait;
use libpijul::output::output_file;
use libpijul::pristine::sanakirja::MutTxn;
use libpijul::pristine::{Hash, Inode, Position};
use libpijul::record::Builder as RecordBuilder;
use libpijul::vertex_buffer::Writer;
use libpijul::working_copy::memory::Memory;
use libpijul::{
    Algorithm, ArcTxn, ChannelRef, ChannelTxnT, Encoding, MutTxnT, MutTxnTExt, Recorded, TreeTxnT,
    TxnT, TxnTExt, DEFAULT_SEPARATOR,
};
use thiserror::Error;

use crate::operations::{Opcode, Operation};

/// Errors that can occur during opcode operations
#[derive(Error, Debug)]
pub enum OpcodeError {
    #[error("Pijul error: {0}")]
    Pijul(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File not found in pristine: {0}")]
    FileNotFound(String),

    #[error("Channel not found: {0}")]
    ChannelNotFound(String),

    #[error("Transaction error: {0}")]
    Transaction(String),

    #[error("Recording error: {0}")]
    Recording(String),

    #[error("Change store error: {0}")]
    ChangeStore(String),

    #[error("Change error: {0}")]
    Change(String),

    #[error("Diff error: {0}")]
    Diff(String),

    #[error("Path conversion error: {0}")]
    PathConversion(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),
}

impl From<ChangeStoreError> for OpcodeError {
    fn from(e: ChangeStoreError) -> Self {
        OpcodeError::ChangeStore(format!("{:?}", e))
    }
}

impl From<ChangeError> for OpcodeError {
    fn from(e: ChangeError) -> Self {
        OpcodeError::Change(format!("{:?}", e))
    }
}

/// Backend for applying opcodes to Pijul
///
/// This wraps the PijulBackend with additional functionality for
/// recording opcodes as Pijul changes using direct diffing.
pub struct OpcodeRecordingBackend {
    /// Path to the .pijul directory
    pijul_dir: std::path::PathBuf,
    /// Path to the working directory
    working_dir: std::path::PathBuf,
    /// The pristine database
    pristine: libpijul::pristine::sanakirja::Pristine,
    /// Change store for persisting changes
    changes: ChangeStore,
    /// Current channel name
    current_channel: String,
}

impl OpcodeRecordingBackend {
    /// Create a new OpcodeRecordingBackend from an existing repository
    ///
    /// # Arguments
    /// * `pijul_dir` - Path to the .pijul directory
    /// * `working_dir` - Path to the working directory
    /// * `cache_size` - Number of changes to cache in memory
    pub fn open(
        pijul_dir: &Path,
        working_dir: &Path,
        cache_size: usize,
    ) -> Result<Self, OpcodeError> {
        let db_path = pijul_dir.join("pristine").join("db");

        if !db_path.exists() {
            return Err(OpcodeError::Pijul(format!(
                "Repository not initialized at {:?}",
                pijul_dir
            )));
        }

        let pristine = libpijul::pristine::sanakirja::Pristine::new(&db_path)
            .map_err(|e| OpcodeError::Pijul(format!("{:?}", e)))?;

        let changes_dir = pijul_dir.join("changes");
        let changes = ChangeStore::from_changes(changes_dir, cache_size);

        // Get the current channel from the database
        let current_channel = {
            let txn = pristine
                .txn_begin()
                .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
            txn.current_channel()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| libpijul::DEFAULT_CHANNEL.to_string())
        };

        Ok(Self {
            pijul_dir: pijul_dir.to_path_buf(),
            working_dir: working_dir.to_path_buf(),
            pristine,
            changes,
            current_channel,
        })
    }

    /// Initialize a new repository and return an OpcodeRecordingBackend
    ///
    /// # Arguments
    /// * `pijul_dir` - Path where .pijul contents will be stored
    /// * `working_dir` - Path to the working directory
    /// * `channel` - Optional channel name (defaults to "main")
    /// * `cache_size` - Number of changes to cache in memory
    pub fn init(
        pijul_dir: &Path,
        working_dir: &Path,
        channel: Option<&str>,
        cache_size: usize,
    ) -> Result<Self, OpcodeError> {
        let pristine_dir = pijul_dir.join("pristine");
        let changes_dir = pijul_dir.join("changes");
        let config_path = pijul_dir.join("config");
        let db_path = pristine_dir.join("db");

        // Check if already initialized
        if db_path.exists() {
            return Err(OpcodeError::Pijul(format!(
                "Repository already exists at {:?}",
                pijul_dir
            )));
        }

        // Create directory structure
        std::fs::create_dir_all(&pristine_dir)?;
        std::fs::create_dir_all(&changes_dir)?;
        std::fs::create_dir_all(working_dir)?;

        // Initialize the pristine database
        let pristine = libpijul::pristine::sanakirja::Pristine::new(&db_path)
            .map_err(|e| OpcodeError::Pijul(format!("{:?}", e)))?;

        let channel_name = channel
            .map(String::from)
            .unwrap_or_else(|| libpijul::DEFAULT_CHANNEL.to_string());

        // Create the default channel
        {
            let mut txn = pristine
                .mut_txn_begin()
                .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
            txn.open_or_create_channel(&channel_name)
                .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
            txn.set_current_channel(&channel_name)
                .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
            txn.commit()
                .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        }

        // Write pijul config
        std::fs::write(&config_path, "[hooks]\nrecord = []\n")?;

        let changes = ChangeStore::from_changes(changes_dir, cache_size);

        Ok(Self {
            pijul_dir: pijul_dir.to_path_buf(),
            working_dir: working_dir.to_path_buf(),
            pristine,
            changes,
            current_channel: channel_name,
        })
    }

    /// Apply an opcode and record it as a Pijul change
    ///
    /// Returns the hash of the created change, or None if no change was needed.
    pub fn apply_opcode(&self, opcode: &Opcode) -> Result<Option<Hash>, OpcodeError> {
        let timestamp = opcode.timestamp();

        match opcode.op() {
            Operation::FileWrite { path, offset, data } => {
                self.apply_file_write(path, *offset, data, timestamp)
            }
            Operation::FileCreate {
                path,
                mode,
                content,
            } => self.apply_file_create(path, *mode, content, timestamp),
            Operation::FileTruncate { path, new_size } => {
                self.apply_file_truncate(path, *new_size, timestamp)
            }
            Operation::FileDelete { path } => self.apply_file_delete(path, timestamp),
            // Directory operations, metadata, and links are not yet implemented
            Operation::DirCreate { .. }
            | Operation::DirDelete { .. }
            | Operation::DirRename { .. }
            | Operation::FileRename { .. }
            | Operation::SetPermissions { .. }
            | Operation::SetTimestamps { .. }
            | Operation::SetOwnership { .. }
            | Operation::SymlinkCreate { .. }
            | Operation::SymlinkDelete { .. }
            | Operation::HardLinkCreate { .. } => Err(OpcodeError::UnsupportedOperation(format!(
                "Operation {:?} not yet implemented",
                opcode.op()
            ))),
        }
    }

    /// Apply a FileWrite opcode using direct diffing
    ///
    /// This:
    /// 1. Reads current content from pristine via Graph retrieval
    /// 2. Applies the write in memory to get new bytes
    /// 3. Diffs old Graph vs new bytes directly
    /// 4. Creates and applies the change
    pub fn apply_file_write(
        &self,
        path: &Path,
        offset: u64,
        data: &[u8],
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        let pijul_path = path_to_pijul(path)?;

        // Begin transaction
        let txn = self
            .pristine
            .arc_txn_begin()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        let channel = self.load_channel(&txn)?;

        // Get file position and current content
        let (file_pos, inode) = self.get_file_position(&txn, &channel, &pijul_path)?;
        let mut content = self.get_file_content_at(&txn, &channel, file_pos)?;

        // Apply the write in memory
        let offset = offset as usize;
        let end = offset + data.len();
        if end > content.len() {
            content.resize(end, 0);
        }
        content[offset..end].copy_from_slice(data);

        // Diff and record
        self.diff_and_record(
            txn,
            channel,
            &pijul_path,
            file_pos,
            inode,
            &content,
            &format!("write to {} at offset {}", pijul_path, offset),
            timestamp_ns,
        )
    }

    /// Apply a FileCreate opcode
    ///
    /// For new files, we use the Memory working copy approach since there's
    /// no existing Graph to diff against.
    pub fn apply_file_create(
        &self,
        path: &Path,
        _mode: u32,
        content: &[u8],
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        let pijul_path = path_to_pijul(path)?;

        // Begin transaction
        let txn = self
            .pristine
            .arc_txn_begin()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        let channel = self.load_channel(&txn)?;

        // For a new file, we need to register it in the repository first
        {
            let mut t = txn.write();
            // Add parent directories if they don't exist
            let components: Vec<&str> = pijul_path.split('/').collect();
            let mut current_path = String::new();
            for (i, component) in components.iter().enumerate() {
                if i < components.len() - 1 {
                    // This is a directory
                    if !current_path.is_empty() {
                        current_path.push('/');
                    }
                    current_path.push_str(component);
                    // Try to add directory, ignore if it already exists
                    let _ = t.add_dir(&current_path, 0);
                }
            }
            // Add the file itself
            t.add_file(&pijul_path, 0)
                .map_err(|e| OpcodeError::Transaction(format!("Failed to add file: {:?}", e)))?;
        }

        // For new files, use Memory working copy approach
        // This is necessary because there's no existing Graph to diff against
        self.record_with_memory(
            txn,
            channel,
            &pijul_path,
            content.to_vec(),
            &format!("create file {}", pijul_path),
            timestamp_ns,
        )
    }

    /// Apply a FileTruncate opcode
    pub fn apply_file_truncate(
        &self,
        path: &Path,
        new_size: u64,
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        let pijul_path = path_to_pijul(path)?;

        // Begin transaction
        let txn = self
            .pristine
            .arc_txn_begin()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        let channel = self.load_channel(&txn)?;

        // Get file position and current content
        let (file_pos, inode) = self.get_file_position(&txn, &channel, &pijul_path)?;
        let mut content = self.get_file_content_at(&txn, &channel, file_pos)?;

        // Truncate
        content.truncate(new_size as usize);

        // Diff and record
        self.diff_and_record(
            txn,
            channel,
            &pijul_path,
            file_pos,
            inode,
            &content,
            &format!("truncate {} to {} bytes", pijul_path, new_size),
            timestamp_ns,
        )
    }

    /// Apply a FileDelete opcode
    pub fn apply_file_delete(
        &self,
        path: &Path,
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        let pijul_path = path_to_pijul(path)?;

        // Begin transaction
        let txn = self
            .pristine
            .arc_txn_begin()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        let channel = self.load_channel(&txn)?;

        // Get file position
        let (file_pos, inode) = self.get_file_position(&txn, &channel, &pijul_path)?;

        // For deletion, diff against empty content
        let result = self.diff_and_record(
            txn.clone(),
            channel,
            &pijul_path,
            file_pos,
            inode,
            &[],
            &format!("delete file {}", pijul_path),
            timestamp_ns,
        )?;

        // Also remove from tree tracking
        {
            let mut t = txn.write();
            t.remove_file(&pijul_path)
                .map_err(|e| OpcodeError::Transaction(format!("Failed to remove file: {:?}", e)))?;
        }

        txn.commit()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;

        Ok(result)
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Load the current channel
    fn load_channel(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
    ) -> Result<ChannelRef<MutTxn<()>>, OpcodeError> {
        // Use write() to get access to open_or_create_channel which is on ChannelMutTxnT
        let channel = txn
            .write()
            .open_or_create_channel(&self.current_channel)
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        Ok(channel)
    }

    /// Get file position and inode from path
    fn get_file_position(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
        channel: &ChannelRef<MutTxn<()>>,
        path: &str,
    ) -> Result<(Position<libpijul::pristine::ChangeId>, Inode), OpcodeError> {
        let t = txn.read();
        let (pos, _ambiguous) = (&*t)
            .follow_oldest_path(&self.changes, channel, path)
            .map_err(|_| OpcodeError::FileNotFound(path.to_string()))?;

        // Get the inode for this path from the tree
        let inode = (&*t)
            .get_revinodes(&pos, None)
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?
            .map(|x| *x)
            .unwrap_or(Inode::ROOT);

        Ok((pos, inode))
    }

    /// Get file content at a specific position
    fn get_file_content_at(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
        channel: &ChannelRef<MutTxn<()>>,
        pos: Position<libpijul::pristine::ChangeId>,
    ) -> Result<Vec<u8>, OpcodeError> {
        let mut buffer = Vec::new();
        output_file(
            &self.changes,
            txn,
            channel,
            pos,
            &mut Writer::new(&mut buffer),
        )
        .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        Ok(buffer)
    }

    /// Diff old content against new bytes and record the change
    ///
    /// This is the core function that:
    /// 1. Retrieves the old content as a Graph
    /// 2. Diffs against new bytes
    /// 3. Creates and applies the change
    fn diff_and_record(
        &self,
        txn: ArcTxn<MutTxn<()>>,
        channel: ChannelRef<MutTxn<()>>,
        path: &str,
        file_pos: Position<libpijul::pristine::ChangeId>,
        inode: Inode,
        new_content: &[u8],
        message: &str,
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        // Retrieve the old content as a Graph
        let mut graph = {
            let t = txn.read();
            let c = channel.read();
            alive_retrieve(&*t, (&*t).graph(&*c), file_pos, false)
                .map_err(|e| OpcodeError::Diff(format!("Failed to retrieve graph: {:?}", e)))?
        };

        // Create a Recorded struct for diffing
        let mut recorded = Recorded::new();

        // Perform the diff directly
        // Use text encoding for text files, None for binary
        let encoding = detect_encoding(new_content);

        recorded
            .diff(
                &self.changes,
                &txn,
                &channel,
                Algorithm::default(),
                false, // stop_early
                path.to_string(),
                inode,
                file_pos.to_option(),
                &mut graph,
                new_content,
                &encoding,
                &DEFAULT_SEPARATOR,
            )
            .map_err(|e| OpcodeError::Diff(format!("{:?}", e)))?;

        // Check if anything changed
        if recorded.actions.is_empty() {
            return Ok(None);
        }

        // Create the change header
        let timestamp = jiff::Timestamp::from_nanosecond(timestamp_ns as i128)
            .unwrap_or_else(|_| jiff::Timestamp::now());

        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp,
        };

        // Build the change
        let change = {
            let t = txn.read();
            recorded
                .into_change(&*t, &channel, header)
                .map_err(|e| OpcodeError::Recording(format!("{:?}", e)))?
        };

        // Save to changestore
        let mut change = change;
        let hash = self
            .changes
            .save_change(&mut change, |_, _| Ok::<_, OpcodeError>(()))?;

        // Apply to pristine
        {
            let mut t = txn.write();
            libpijul::apply::apply_local_change(
                &mut *t,
                &channel,
                &change,
                &hash,
                &std::collections::HashMap::new(),
            )
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        }

        // Commit transaction
        txn.commit()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;

        Ok(Some(hash))
    }

    /// Record a change using Memory working copy
    ///
    /// This is used for new file creation where there's no existing Graph to diff against.
    fn record_with_memory(
        &self,
        txn: ArcTxn<MutTxn<()>>,
        channel: ChannelRef<MutTxn<()>>,
        path: &str,
        content: Vec<u8>,
        message: &str,
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, OpcodeError> {
        // Create memory working copy
        let memory = Memory::new();

        // Populate directory structure
        let mut current = String::new();
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        for (i, component) in components.iter().enumerate() {
            if i < components.len() - 1 {
                if !current.is_empty() {
                    current.push('/');
                }
                current.push_str(component);
                memory.add_dir(&current);
            }
        }

        // Add the file with new content
        memory.add_file(path, content);

        // Build and record the change
        let mut builder = RecordBuilder::new();

        builder
            .record(
                txn.clone(),
                Algorithm::default(),
                false, // stop_early
                &DEFAULT_SEPARATOR,
                channel.clone(),
                &memory,
                &self.changes,
                path,
                1, // single-threaded
            )
            .map_err(|e| OpcodeError::Recording(format!("{:?}", e)))?;

        let recorded = builder.finish();

        // Check if anything changed
        if recorded.actions.is_empty() {
            return Ok(None);
        }

        // Create the change header
        let timestamp = jiff::Timestamp::from_nanosecond(timestamp_ns as i128)
            .unwrap_or_else(|_| jiff::Timestamp::now());

        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp,
        };

        // Build the change
        let change = {
            let t = txn.read();
            recorded
                .into_change(&*t, &channel, header)
                .map_err(|e| OpcodeError::Recording(format!("{:?}", e)))?
        };

        // Save to changestore
        let mut change = change;
        let hash = self
            .changes
            .save_change(&mut change, |_, _| Ok::<_, OpcodeError>(()))?;

        // Apply to pristine
        {
            let mut t = txn.write();
            libpijul::apply::apply_local_change(
                &mut *t,
                &channel,
                &change,
                &hash,
                &std::collections::HashMap::new(),
            )
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;
        }

        // Commit transaction
        txn.commit()
            .map_err(|e| OpcodeError::Transaction(format!("{:?}", e)))?;

        Ok(Some(hash))
    }

    /// Get the current channel name
    pub fn current_channel(&self) -> &str {
        &self.current_channel
    }

    /// Get the path to the pijul directory
    pub fn pijul_dir(&self) -> &Path {
        &self.pijul_dir
    }

    /// Get the path to the working directory
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }
}

/// Convert a filesystem path to Pijul's path format
///
/// Pijul uses `/`-separated paths without leading slashes or `./` prefix.
fn path_to_pijul(path: &Path) -> Result<String, OpcodeError> {
    let path_str = path.to_string_lossy();
    let cleaned = path_str
        .trim_start_matches("./")
        .trim_start_matches('/')
        .replace(std::path::MAIN_SEPARATOR, "/");

    if cleaned.is_empty() {
        return Err(OpcodeError::PathConversion(
            "Empty path after conversion".to_string(),
        ));
    }

    Ok(cleaned)
}

/// Detect text encoding from content
///
/// Returns Some(Encoding) for text files, None for binary
fn detect_encoding(content: &[u8]) -> Option<Encoding> {
    // Simple heuristic: if content contains null bytes, treat as binary
    if content.contains(&0) {
        None
    } else {
        // Assume UTF-8 for text
        Some(Encoding::for_label("utf-8"))
    }
}

impl std::fmt::Debug for OpcodeRecordingBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcodeRecordingBackend")
            .field("pijul_dir", &self.pijul_dir)
            .field("working_dir", &self.working_dir)
            .field("current_channel", &self.current_channel)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, OpcodeRecordingBackend) {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = OpcodeRecordingBackend::init(&pijul_dir, &working_dir, None, 100).unwrap();

        (temp, backend)
    }

    #[test]
    fn test_path_to_pijul() {
        assert_eq!(path_to_pijul(Path::new("foo.txt")).unwrap(), "foo.txt");
        assert_eq!(path_to_pijul(Path::new("./foo.txt")).unwrap(), "foo.txt");
        assert_eq!(path_to_pijul(Path::new("/foo.txt")).unwrap(), "foo.txt");
        assert_eq!(
            path_to_pijul(Path::new("dir/foo.txt")).unwrap(),
            "dir/foo.txt"
        );
        assert_eq!(
            path_to_pijul(Path::new("./dir/foo.txt")).unwrap(),
            "dir/foo.txt"
        );
    }

    #[test]
    fn test_init_and_open() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        // Initialize
        let backend = OpcodeRecordingBackend::init(&pijul_dir, &working_dir, None, 100).unwrap();
        assert_eq!(backend.current_channel(), "main");

        // Open existing
        drop(backend);
        let backend = OpcodeRecordingBackend::open(&pijul_dir, &working_dir, 100).unwrap();
        assert_eq!(backend.current_channel(), "main");
    }

    #[test]
    fn test_file_create() {
        let (_temp, backend) = setup_test_repo();

        let opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello, world!".to_vec(),
            },
        );

        let result = backend.apply_opcode(&opcode);
        if let Err(ref e) = result {
            eprintln!("Error: {:?}", e);
        }
        assert!(result.is_ok(), "apply_opcode failed: {:?}", result.err());
        // First file creation should create a change
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_unsupported_operations() {
        let (_temp, backend) = setup_test_repo();

        let opcode = Opcode::new(
            1,
            Operation::DirCreate {
                path: std::path::PathBuf::from("testdir"),
                mode: 0o755,
            },
        );

        let result = backend.apply_opcode(&opcode);
        assert!(matches!(result, Err(OpcodeError::UnsupportedOperation(_))));
    }

    #[test]
    fn test_file_write_to_existing() {
        let (_temp, backend) = setup_test_repo();

        // First create a file
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello, world!".to_vec(),
            },
        );
        let result = backend.apply_opcode(&create_opcode);
        assert!(result.is_ok(), "Failed to create file: {:?}", result.err());

        // Now write to the middle of the file
        let write_opcode = Opcode::new(
            2,
            Operation::FileWrite {
                path: std::path::PathBuf::from("test.txt"),
                offset: 7,
                data: b"Rust".to_vec(),
            },
        );
        let result = backend.apply_opcode(&write_opcode);
        assert!(
            result.is_ok(),
            "Failed to write to file: {:?}",
            result.err()
        );
        // Write should create a change
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_file_write_extends_file() {
        let (_temp, backend) = setup_test_repo();

        // Create a file
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello".to_vec(),
            },
        );
        backend.apply_opcode(&create_opcode).unwrap();

        // Write beyond the current end - should extend the file
        let write_opcode = Opcode::new(
            2,
            Operation::FileWrite {
                path: std::path::PathBuf::from("test.txt"),
                offset: 10,
                data: b"World".to_vec(),
            },
        );
        let result = backend.apply_opcode(&write_opcode);
        assert!(result.is_ok(), "Failed to extend file: {:?}", result.err());
    }

    #[test]
    fn test_file_truncate() {
        let (_temp, backend) = setup_test_repo();

        // Create a file with content
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello, world! This is a longer string.".to_vec(),
            },
        );
        backend.apply_opcode(&create_opcode).unwrap();

        // Truncate to a shorter length
        let truncate_opcode = Opcode::new(
            2,
            Operation::FileTruncate {
                path: std::path::PathBuf::from("test.txt"),
                new_size: 5,
            },
        );
        let result = backend.apply_opcode(&truncate_opcode);
        assert!(
            result.is_ok(),
            "Failed to truncate file: {:?}",
            result.err()
        );
        // Truncate should create a change
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_file_in_subdirectory() {
        let (_temp, backend) = setup_test_repo();

        // Create a file in a subdirectory
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("subdir/nested/test.txt"),
                mode: 0o644,
                content: b"Content in nested directory".to_vec(),
            },
        );
        let result = backend.apply_opcode(&create_opcode);
        assert!(
            result.is_ok(),
            "Failed to create file in subdirectory: {:?}",
            result.err()
        );
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_write_same_content() {
        let (_temp, backend) = setup_test_repo();

        // Create a file
        let create_opcode = Opcode::new(
            1,
            Operation::FileCreate {
                path: std::path::PathBuf::from("test.txt"),
                mode: 0o644,
                content: b"Hello".to_vec(),
            },
        );
        backend.apply_opcode(&create_opcode).unwrap();

        // Write the same content - pijul may or may not create a change
        // depending on internal state. The important thing is it doesn't error.
        let write_opcode = Opcode::new(
            2,
            Operation::FileWrite {
                path: std::path::PathBuf::from("test.txt"),
                offset: 0,
                data: b"Hello".to_vec(),
            },
        );
        let result = backend.apply_opcode(&write_opcode);
        assert!(
            result.is_ok(),
            "Failed to write same content: {:?}",
            result.err()
        );
        // Result could be Some (change recorded) or None (no diff detected)
        // Both are valid outcomes
    }
}
