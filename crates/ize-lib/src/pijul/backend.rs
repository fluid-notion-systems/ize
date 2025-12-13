//! PijulBackend - Core interface to libpijul
//!
//! This module provides `PijulBackend`, which is the single source of truth
//! for all Pijul operations in Ize. It wraps libpijul and provides a high-level
//! API for recording file changes and querying repository state.

use log::debug;
use std::path::{Path, PathBuf};

use libpijul::alive_retrieve;
use libpijul::change::{ChangeError, ChangeHeader};
use libpijul::changestore::filesystem::{Error as ChangeStoreError, FileSystem as ChangeStore};
use libpijul::changestore::ChangeStore as ChangeStoreTrait;
use libpijul::output::output_file;
use libpijul::pristine::sanakirja::{MutTxn, Pristine, SanakirjaError, Txn};
use libpijul::pristine::{Hash, Inode, Position};
use libpijul::record::Builder as RecordBuilder;
use libpijul::vertex_buffer::Writer;
use libpijul::working_copy::filesystem::FileSystem as WorkingCopy;
use libpijul::working_copy::memory::Memory;
use libpijul::{
    Algorithm, ArcTxn, ChannelRef, ChannelTxnT, Encoding, MutTxnT, MutTxnTExt, Recorded, TreeTxnT,
    TxnT, TxnTExt, DEFAULT_SEPARATOR,
};
use thiserror::Error;

/// Constants matching pijul-repository
pub const PRISTINE_DIR: &str = "pristine";
pub const CHANGES_DIR: &str = "changes";
pub const CONFIG_FILE: &str = "config";
pub const DB_FILE: &str = "db";

/// Default initial size for the pristine database (1MB)
pub const DEFAULT_PRISTINE_SIZE: u64 = 1 << 20;

#[derive(Error, Debug)]
pub enum PijulError {
    #[error("Sanakirja database error: {0}")]
    Sanakirja(#[from] SanakirjaError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Repository not initialized at {0}")]
    NotInitialized(PathBuf),

    #[error("Repository already exists at {0}")]
    AlreadyExists(PathBuf),

    #[error("Channel not found: {0}")]
    ChannelNotFound(String),

    #[error("Transaction error: {0}")]
    Transaction(String),

    #[error("Change store error: {0}")]
    ChangeStore(String),

    #[error("Fork error: {0}")]
    Fork(String),

    #[error("File not found in pristine: {0}")]
    FileNotFound(String),

    #[error("Recording error: {0}")]
    Recording(String),

    #[error("Diff error: {0}")]
    Diff(String),

    #[error("Path conversion error: {0}")]
    PathConversion(String),
}

impl From<ChangeStoreError> for PijulError {
    fn from(e: ChangeStoreError) -> Self {
        PijulError::ChangeStore(format!("{:?}", e))
    }
}

impl From<ChangeError> for PijulError {
    fn from(e: ChangeError) -> Self {
        PijulError::Recording(format!("{:?}", e))
    }
}

/// Wrapper around libpijul for Ize's custom directory structure.
///
/// Unlike standard pijul where `.pijul/` is inside the working directory,
/// Ize keeps `.pijul/` and `working/` as siblings:
///
/// ```text
/// {project}/
/// ├── .pijul/      <- pijul_dir
/// ├── working/     <- working_dir
/// └── meta/
/// ```
pub struct PijulBackend {
    /// Path to the .pijul directory
    pijul_dir: PathBuf,
    /// Path to the working directory (sibling of .pijul)
    working_dir: PathBuf,
    /// The pristine database handle
    pristine: Pristine,
    /// Current channel name
    current_channel: String,
}

impl PijulBackend {
    /// Initialize a new pijul repository at the given paths.
    ///
    /// This creates:
    /// - `{pijul_dir}/pristine/db` - The sanakirja database
    /// - `{pijul_dir}/changes/` - Directory for change files
    /// - `{pijul_dir}/config` - Pijul config file
    /// - Default "main" channel
    ///
    /// # Arguments
    /// * `pijul_dir` - Path where `.pijul/` contents will be stored
    /// * `working_dir` - Path to the working directory
    /// * `channel` - Optional channel name (defaults to "main")
    pub fn init(
        pijul_dir: &Path,
        working_dir: &Path,
        channel: Option<&str>,
    ) -> Result<Self, PijulError> {
        let pristine_dir = pijul_dir.join(PRISTINE_DIR);
        let changes_dir = pijul_dir.join(CHANGES_DIR);
        let config_path = pijul_dir.join(CONFIG_FILE);
        let db_path = pristine_dir.join(DB_FILE);

        // Check if already initialized
        if db_path.exists() {
            return Err(PijulError::AlreadyExists(pijul_dir.to_path_buf()));
        }

        // Create directory structure
        std::fs::create_dir_all(&pristine_dir)?;
        std::fs::create_dir_all(&changes_dir)?;
        std::fs::create_dir_all(working_dir)?;

        // Initialize the pristine database
        // Note: Pristine::new expects the path to the db file, not the directory
        let pristine = Pristine::new(&db_path)?;

        let channel_name = channel
            .map(String::from)
            .unwrap_or_else(|| libpijul::DEFAULT_CHANNEL.to_string());

        // Create the default channel
        {
            let mut txn = pristine.mut_txn_begin()?;
            txn.open_or_create_channel(&channel_name)?;
            txn.set_current_channel(&channel_name)?;
            txn.commit()?;
        }

        // Write pijul config (matching pijul's init_default_config)
        std::fs::write(&config_path, "[hooks]\nrecord = []\n")?;

        Ok(Self {
            pijul_dir: pijul_dir.to_path_buf(),
            working_dir: working_dir.to_path_buf(),
            pristine,
            current_channel: channel_name,
        })
    }

    /// Open an existing pijul repository.
    ///
    /// # Arguments
    /// * `pijul_dir` - Path to the `.pijul/` directory
    /// * `working_dir` - Path to the working directory
    pub fn open(pijul_dir: &Path, working_dir: &Path) -> Result<Self, PijulError> {
        let db_path = pijul_dir.join(PRISTINE_DIR).join(DB_FILE);

        if !db_path.exists() {
            return Err(PijulError::NotInitialized(pijul_dir.to_path_buf()));
        }

        let pristine = Pristine::new(&db_path)?;

        // Get the current channel from the database
        let current_channel = {
            let txn = pristine.txn_begin()?;
            txn.current_channel()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| libpijul::DEFAULT_CHANNEL.to_string())
        };

        Ok(Self {
            pijul_dir: pijul_dir.to_path_buf(),
            working_dir: working_dir.to_path_buf(),
            pristine,
            current_channel,
        })
    }

    /// Get the path to the .pijul directory
    pub fn pijul_dir(&self) -> &Path {
        &self.pijul_dir
    }

    /// Get the path to the working directory
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Get the path to the changes directory
    pub fn changes_dir(&self) -> PathBuf {
        self.pijul_dir.join(CHANGES_DIR)
    }

    /// Get the current channel name
    pub fn current_channel(&self) -> &str {
        &self.current_channel
    }

    /// Get a reference to the pristine database
    pub fn pristine(&self) -> &Pristine {
        &self.pristine
    }

    /// Create a working copy filesystem handle
    pub fn working_copy(&self) -> WorkingCopy {
        WorkingCopy::from_root(&self.working_dir)
    }

    // === Channel Operations ===

    /// Create a new channel (like a branch)
    pub fn create_channel(&self, name: &str) -> Result<(), PijulError> {
        let mut txn = self.pristine.mut_txn_begin()?;
        txn.open_or_create_channel(name)?;
        txn.commit()?;
        Ok(())
    }

    /// Switch to a different channel
    pub fn switch_channel(&mut self, name: &str) -> Result<(), PijulError> {
        let mut txn = self.pristine.mut_txn_begin()?;

        // Verify channel exists
        let channel = txn
            .load_channel(name)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;

        if channel.is_none() {
            return Err(PijulError::ChannelNotFound(name.to_string()));
        }

        txn.set_current_channel(name)?;
        txn.commit()?;
        self.current_channel = name.to_string();
        Ok(())
    }

    /// List all channels in the repository
    pub fn list_channels(&self) -> Result<Vec<String>, PijulError> {
        let txn = self.pristine.txn_begin()?;
        let mut channels = Vec::new();

        let channel_refs = txn
            .channels("")
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;

        for channel_ref in channel_refs {
            let channel = channel_ref.read();
            channels.push(txn.name(&*channel).to_string());
        }

        Ok(channels)
    }

    /// Fork a channel (create a new channel from an existing one)
    pub fn fork_channel(&self, from: &str, to: &str) -> Result<(), PijulError> {
        let mut txn = self.pristine.mut_txn_begin()?;

        let from_channel = txn
            .load_channel(from)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
            .ok_or_else(|| PijulError::ChannelNotFound(from.to_string()))?;

        txn.fork(&from_channel, to)
            .map_err(|e| PijulError::Fork(format!("{:?}", e)))?;
        txn.commit()?;
        Ok(())
    }

    // === Transaction Helpers ===

    /// Begin a read-only transaction
    pub fn txn_begin(&self) -> Result<Txn, PijulError> {
        Ok(self.pristine.txn_begin()?)
    }

    /// Begin a mutable transaction
    pub fn mut_txn_begin(&self) -> Result<MutTxn<()>, PijulError> {
        Ok(self.pristine.mut_txn_begin()?)
    }

    /// Begin a thread-safe transaction (for concurrent access)
    pub fn arc_txn_begin(&self) -> Result<libpijul::ArcTxn<MutTxn<()>>, PijulError> {
        Ok(self.pristine.arc_txn_begin()?)
    }

    // === File Operations (High-Level API for Opcode Processing) ===

    /// Record creation of a new file
    ///
    /// This is a high-level API that handles all the complexity of recording
    /// a new file to Pijul, including:
    /// - Creating parent directories as needed
    /// - Adding the file to the tree
    /// - Recording the change using Memory working copy
    /// - Saving to the change store
    /// - Applying to pristine
    ///
    /// # Arguments
    /// * `path` - File path (e.g. "src/main.rs")
    /// * `mode` - File mode (currently unused by libpijul)
    /// * `content` - Initial file content
    /// * `message` - Commit message
    ///
    /// # Returns
    /// The hash of the created change, or None if there were no changes to record
    pub fn record_file_create(
        &self,
        path: &str,
        _mode: u32,
        content: &[u8],
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!(
            "PijulBackend::record_file_create path={:?} content_len={}",
            path,
            content.len()
        );
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Add file to tree (and parent directories)
        {
            let mut t = txn.write();
            // Add parent directories if they don't exist
            let components: Vec<&str> = path.split('/').collect();
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
            t.add_file(path, 0)
                .map_err(|e| PijulError::Transaction(format!("Failed to add file: {:?}", e)))?;
        }

        // Use Memory working copy for new files
        self.record_with_memory(txn, channel, path, content.to_vec(), message)
    }

    /// Record modification to an existing file
    ///
    /// This handles writing data to an existing file at a specific offset.
    /// It retrieves the current content, applies the write operation, and
    /// records the diff.
    ///
    /// # Arguments
    /// * `path` - File path
    /// * `offset` - Byte offset to write at
    /// * `data` - Data to write
    /// * `message` - Commit message
    ///
    /// # Returns
    /// The hash of the created change, or None if there were no changes to record
    pub fn record_file_write(
        &self,
        path: &str,
        offset: u64,
        data: &[u8],
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!(
            "PijulBackend::record_file_write path={:?} offset={} data_len={}",
            path,
            offset,
            data.len()
        );
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Get file position and current content
        let (file_pos, inode) = self.get_file_position(&txn, &channel, path)?;
        let mut content = self.get_file_content_at(&txn, &channel, file_pos)?;

        // Apply the write operation
        let offset = offset as usize;
        if offset > content.len() {
            // Extend with zeros if writing beyond current size
            content.resize(offset, 0);
        }

        // Replace or append data
        if offset + data.len() > content.len() {
            content.resize(offset + data.len(), 0);
        }
        content[offset..offset + data.len()].copy_from_slice(data);

        // Diff and record
        self.diff_and_record(txn, channel, path, file_pos, inode, &content, message)
    }

    /// Record file truncation
    ///
    /// # Arguments
    /// * `path` - File path
    /// * `new_size` - New size in bytes
    /// * `message` - Commit message
    ///
    /// # Returns
    /// The hash of the created change, or None if there were no changes to record
    pub fn record_file_truncate(
        &self,
        path: &str,
        new_size: u64,
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!(
            "PijulBackend::record_file_truncate path={:?} new_size={}",
            path, new_size
        );
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Get file position and current content
        let (file_pos, inode) = self.get_file_position(&txn, &channel, path)?;
        let mut content = self.get_file_content_at(&txn, &channel, file_pos)?;

        // Truncate
        content.truncate(new_size as usize);

        // Diff and record
        self.diff_and_record(txn, channel, path, file_pos, inode, &content, message)
    }

    /// Record file deletion
    ///
    /// # Arguments
    /// * `path` - File path
    /// * `message` - Commit message
    ///
    /// # Returns
    /// The hash of the created change, or None if there were no changes to record
    pub fn record_file_delete(
        &self,
        path: &str,
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!("PijulBackend::record_file_delete path={:?}", path);
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Get file position
        let (file_pos, inode) = self.get_file_position(&txn, &channel, path)?;

        // Remove from tree tracking first (before the diff)
        {
            let mut t = txn.write();
            t.remove_file(path)
                .map_err(|e| PijulError::Transaction(format!("Failed to remove file: {:?}", e)))?;
        }

        // For deletion, diff against empty content
        // Note: diff_and_record takes ownership and commits the transaction
        let result = self.diff_and_record(txn, channel, path, file_pos, inode, &[], message)?;

        Ok(result)
    }

    /// Record file rename
    ///
    /// # Arguments
    /// * `old_path` - Current file path
    /// * `new_path` - New file path
    /// * `message` - Commit message
    ///
    /// # Returns
    /// The hash of the created change
    pub fn record_file_rename(
        &self,
        old_path: &str,
        new_path: &str,
        message: &str,
    ) -> Result<Hash, PijulError> {
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Move the file in the tree
        {
            let mut t = txn.write();
            t.move_file(old_path, new_path, 0)
                .map_err(|e| PijulError::Transaction(format!("Failed to move file: {:?}", e)))?;
        }

        // Record the change
        let memory = Memory::new();
        let mut builder = RecordBuilder::new();

        builder
            .record(
                txn.clone(),
                Algorithm::default(),
                false,
                &DEFAULT_SEPARATOR,
                channel.clone(),
                &memory,
                &self.get_change_store(),
                "",
                1,
            )
            .map_err(|e| PijulError::Recording(format!("{:?}", e)))?;

        let recorded = builder.finish();

        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp: jiff::Timestamp::now(),
        };

        let change = {
            let t = txn.read();
            recorded
                .into_change(&*t, &channel, header)
                .map_err(|e| PijulError::Recording(format!("{:?}", e)))?
        };

        let mut change = change;
        let change_store = self.get_change_store();
        let hash = change_store.save_change(&mut change, |_, _| Ok::<_, ChangeStoreError>(()))?;

        {
            let mut t = txn.write();
            libpijul::apply::apply_local_change(
                &mut *t,
                &channel,
                &change,
                &hash,
                &std::collections::HashMap::new(),
            )
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
        }

        txn.commit()
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;

        Ok(hash)
    }

    // === Query Operations (for Reading Pijul State) ===

    /// Get file content at current channel head
    ///
    /// # Arguments
    /// * `path` - File path
    ///
    /// # Returns
    /// The file content as bytes
    pub fn get_file_content(&self, path: &str) -> Result<Vec<u8>, PijulError> {
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        // Get file position using follow_oldest_path
        let (file_pos, _ambiguous) = {
            let t = txn.read();
            (&*t)
                .follow_oldest_path(&self.get_change_store(), &channel, path)
                .map_err(|_| PijulError::FileNotFound(path.to_string()))?
        };

        // Retrieve content using output_file
        let mut buffer = Vec::new();
        output_file(
            &self.get_change_store(),
            &txn,
            &channel,
            file_pos,
            &mut Writer::new(&mut buffer),
        )
        .map_err(|e| PijulError::Diff(format!("Failed to output file: {:?}", e)))?;

        Ok(buffer)
    }

    /// Check if file exists in current channel
    ///
    /// # Arguments
    /// * `path` - File path
    ///
    /// # Returns
    /// true if the file exists, false otherwise
    pub fn file_exists(&self, path: &str) -> Result<bool, PijulError> {
        let txn = self.arc_txn_begin()?;
        let channel = self.load_channel_ref(&txn)?;

        let t = txn.read();
        let result = (&*t).follow_oldest_path(&self.get_change_store(), &channel, path);
        Ok(result.is_ok())
    }

    /// List all files in current channel
    ///
    /// # Returns
    /// A vector of file paths
    pub fn list_files(&self) -> Result<Vec<String>, PijulError> {
        let _txn = self.arc_txn_begin()?;
        let _channel = self.load_channel_ref(&_txn)?;
        let files = Vec::new();

        // This is a simplified implementation - in a real scenario you'd walk the tree
        // For now, just return an empty list (this can be improved later)
        // TODO: Implement proper file listing from pristine

        Ok(files)
    }

    /// List all changes in current channel
    ///
    /// # Returns
    /// A vector of change hashes in chronological order
    pub fn list_changes(&self) -> Result<Vec<Hash>, PijulError> {
        let txn = self.txn_begin()?;
        let channel = txn
            .load_channel(&self.current_channel)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
            .ok_or_else(|| PijulError::ChannelNotFound(self.current_channel.clone()))?;

        let channel_ref = channel.read();
        let mut changes = Vec::new();

        // Iterate through the channel's log
        for entry in txn
            .log(&*channel_ref, 0)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
        {
            let (_, (hash_ref, _)) =
                entry.map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
            changes.push((*hash_ref).into());
        }

        Ok(changes)
    }

    // === Internal Helper Methods ===

    /// Load a channel reference from a transaction
    fn load_channel_ref(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
    ) -> Result<ChannelRef<MutTxn<()>>, PijulError> {
        let t = txn.read();
        let channel = t
            .load_channel(&self.current_channel)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
            .ok_or_else(|| PijulError::ChannelNotFound(self.current_channel.clone()))?;
        Ok(channel)
    }

    /// Get file position and inode for a given path
    fn get_file_position(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
        channel: &ChannelRef<MutTxn<()>>,
        path: &str,
    ) -> Result<(Position<libpijul::pristine::ChangeId>, Inode), PijulError> {
        let t = txn.read();

        let (pos, _ambiguous) = (&*t)
            .follow_oldest_path(&self.get_change_store(), channel, path)
            .map_err(|_| PijulError::FileNotFound(path.to_string()))?;

        // Get the inode for this path from the tree
        let inode = (&*t)
            .get_revinodes(&pos, None)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
            .map(|x| *x)
            .unwrap_or(Inode::ROOT);

        Ok((pos, inode))
    }

    /// Get file content at a specific position
    fn get_file_content_at(
        &self,
        txn: &ArcTxn<MutTxn<()>>,
        channel: &ChannelRef<MutTxn<()>>,
        file_pos: Position<libpijul::pristine::ChangeId>,
    ) -> Result<Vec<u8>, PijulError> {
        let mut buffer = Vec::new();
        output_file(
            &self.get_change_store(),
            txn,
            channel,
            file_pos,
            &mut Writer::new(&mut buffer),
        )
        .map_err(|e| PijulError::Diff(format!("Failed to output file: {:?}", e)))?;
        Ok(buffer)
    }

    /// Diff old content against new content and record the change
    fn diff_and_record(
        &self,
        txn: ArcTxn<MutTxn<()>>,
        channel: ChannelRef<MutTxn<()>>,
        path: &str,
        file_pos: Position<libpijul::pristine::ChangeId>,
        inode: Inode,
        new_content: &[u8],
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!(
            "PijulBackend::diff_and_record path={:?} new_content_len={}",
            path,
            new_content.len()
        );
        // Retrieve the old content as a Graph
        let mut graph = {
            let t = txn.read();
            let c = channel.read();
            alive_retrieve(&*t, (&*t).graph(&*c), file_pos, false)
                .map_err(|e| PijulError::Diff(format!("Failed to retrieve graph: {:?}", e)))?
        };

        // Create a Recorded struct for diffing
        let mut recorded = Recorded::new();

        // Perform the diff directly
        let encoding = detect_encoding(new_content);

        recorded
            .diff(
                &self.get_change_store(),
                &txn,
                &channel,
                Algorithm::default(),
                false,
                path.to_string(),
                inode,
                file_pos.to_option(),
                &mut graph,
                new_content,
                &encoding,
                &DEFAULT_SEPARATOR,
            )
            .map_err(|e| PijulError::Diff(format!("{:?}", e)))?;

        // Check if anything changed
        if recorded.actions.is_empty() {
            debug!("PijulBackend::diff_and_record: no actions, returning None");
            return Ok(None);
        }
        debug!(
            "PijulBackend::diff_and_record: {} actions to record",
            recorded.actions.len()
        );

        // Create the change header
        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp: jiff::Timestamp::now(),
        };

        // Build the change
        let change = {
            let t = txn.read();
            recorded
                .into_change(&*t, &channel, header)
                .map_err(|e| PijulError::Recording(format!("{:?}", e)))?
        };

        // Save to changestore
        let mut change = change;
        let change_store = self.get_change_store();
        let hash = change_store.save_change(&mut change, |_, _| Ok::<_, ChangeStoreError>(()))?;

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
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
        }

        // Commit transaction
        txn.commit()
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;

        debug!("PijulBackend::diff_and_record: committed hash={:?}", hash);
        Ok(Some(hash))
    }

    /// Record a change using Memory working copy (for new files)
    fn record_with_memory(
        &self,
        txn: ArcTxn<MutTxn<()>>,
        channel: ChannelRef<MutTxn<()>>,
        path: &str,
        content: Vec<u8>,
        message: &str,
    ) -> Result<Option<Hash>, PijulError> {
        debug!(
            "PijulBackend::record_with_memory path={:?} content_len={}",
            path,
            content.len()
        );
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
                false,
                &DEFAULT_SEPARATOR,
                channel.clone(),
                &memory,
                &self.get_change_store(),
                path,
                1,
            )
            .map_err(|e| PijulError::Recording(format!("{:?}", e)))?;

        let recorded = builder.finish();

        // Check if anything changed
        if recorded.actions.is_empty() {
            debug!("PijulBackend::record_with_memory: no actions, returning None");
            return Ok(None);
        }
        debug!(
            "PijulBackend::record_with_memory: {} actions to record",
            recorded.actions.len()
        );

        // Create the change header
        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp: jiff::Timestamp::now(),
        };

        // Build the change
        let change = {
            let t = txn.read();
            recorded
                .into_change(&*t, &channel, header)
                .map_err(|e| PijulError::Recording(format!("{:?}", e)))?
        };

        // Save to changestore
        let mut change = change;
        let change_store = self.get_change_store();
        let hash = change_store.save_change(&mut change, |_, _| Ok::<_, ChangeStoreError>(()))?;

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
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
        }

        // Commit transaction
        txn.commit()
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;

        debug!(
            "PijulBackend::record_with_memory: committed hash={:?}",
            hash
        );
        Ok(Some(hash))
    }

    /// Get a change store handle
    pub fn get_change_store(&self) -> ChangeStore {
        let changes_dir = self.changes_dir();
        ChangeStore::from_changes(changes_dir, 1024)
    }
}

/// Detect encoding for a file based on its content
fn detect_encoding(content: &[u8]) -> Option<Encoding> {
    // Simple heuristic: if it contains null bytes, treat as binary
    if content.contains(&0) {
        None
    } else {
        // Assume UTF-8 for text
        Some(Encoding::for_label("utf-8"))
    }
}

impl std::fmt::Debug for PijulBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PijulBackend")
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

    #[test]
    fn test_init_and_open() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        // Initialize
        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        assert_eq!(backend.current_channel(), "main");
        assert!(pijul_dir.join("pristine/db").exists());
        assert!(pijul_dir.join("changes").exists());
        assert!(pijul_dir.join("config").exists());

        // Open existing
        drop(backend);
        let backend = PijulBackend::open(&pijul_dir, &working_dir).unwrap();
        assert_eq!(backend.current_channel(), "main");
    }

    #[test]
    fn test_custom_channel() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, Some("dev")).unwrap();
        assert_eq!(backend.current_channel(), "dev");

        let channels = backend.list_channels().unwrap();
        assert!(channels.contains(&"dev".to_string()));
    }

    #[test]
    fn test_channel_operations() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let mut backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create a new channel
        backend.create_channel("feature").unwrap();

        // List channels
        let channels = backend.list_channels().unwrap();
        assert!(channels.contains(&"main".to_string()));
        assert!(channels.contains(&"feature".to_string()));

        // Switch channel
        backend.switch_channel("feature").unwrap();
        assert_eq!(backend.current_channel(), "feature");

        // Switch to non-existent channel should fail
        assert!(backend.switch_channel("nonexistent").is_err());
    }

    #[test]
    fn test_already_exists_error() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Second init should fail
        let result = PijulBackend::init(&pijul_dir, &working_dir, None);
        assert!(matches!(result, Err(PijulError::AlreadyExists(_))));
    }

    #[test]
    fn test_not_initialized_error() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let result = PijulBackend::open(&pijul_dir, &working_dir);
        assert!(matches!(result, Err(PijulError::NotInitialized(_))));
    }

    #[test]
    fn test_record_file_create() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create a new file
        let content = b"Hello, World!";
        let hash = backend
            .record_file_create("test.txt", 0, content, "Create test.txt")
            .unwrap();

        assert!(hash.is_some(), "Should return a hash for the created file");

        // Verify file exists
        assert!(backend.file_exists("test.txt").unwrap());

        // Verify content
        let retrieved = backend.get_file_content("test.txt").unwrap();
        assert_eq!(retrieved, content);

        // Verify change was recorded
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0], hash.unwrap());
    }

    #[test]
    fn test_record_file_write() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create initial file
        backend
            .record_file_create("test.txt", 0, b"Hello, World!", "Create test.txt")
            .unwrap();

        // Write to the file (overwrite "World" with "Pijul")
        let hash = backend
            .record_file_write("test.txt", 7, b"Pijul", "Update test.txt")
            .unwrap();

        assert!(hash.is_some(), "Should return a hash for the write");

        // Verify content
        let retrieved = backend.get_file_content("test.txt").unwrap();
        assert_eq!(retrieved, b"Hello, Pijul!");

        // Verify we have 2 changes now
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_record_file_truncate() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create initial file
        backend
            .record_file_create("test.txt", 0, b"Hello, World!", "Create test.txt")
            .unwrap();

        // Truncate to 5 bytes
        let hash = backend
            .record_file_truncate("test.txt", 5, "Truncate test.txt")
            .unwrap();

        assert!(hash.is_some(), "Should return a hash for the truncate");

        // Verify content
        let retrieved = backend.get_file_content("test.txt").unwrap();
        assert_eq!(retrieved, b"Hello");

        // Verify we have 2 changes
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_record_file_delete() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create initial file
        backend
            .record_file_create("test.txt", 0, b"Hello, World!", "Create test.txt")
            .unwrap();

        // Delete the file
        let hash = backend
            .record_file_delete("test.txt", "Delete test.txt")
            .unwrap();

        assert!(hash.is_some(), "Should return a hash for the delete");

        // Verify we have 2 changes (create + delete)
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 2);

        // Verify that getting the content returns empty (deleted file has no content)
        let content = backend.get_file_content("test.txt").unwrap();
        assert!(content.is_empty(), "Deleted file should have empty content");
    }

    #[test]
    fn test_file_with_subdirectory() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create a file in a subdirectory
        let content = b"Nested file content";
        let hash = backend
            .record_file_create("src/main.rs", 0, content, "Create src/main.rs")
            .unwrap();

        assert!(hash.is_some());

        // Verify file exists
        assert!(backend.file_exists("src/main.rs").unwrap());

        // Verify content
        let retrieved = backend.get_file_content("src/main.rs").unwrap();
        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_no_change_on_same_content() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create initial file
        let content = b"Hello, World!";
        backend
            .record_file_create("test.txt", 0, content, "Create test.txt")
            .unwrap();

        // Write the same content again
        let hash = backend
            .record_file_write("test.txt", 0, content, "Write same content")
            .unwrap();

        // Should return None because there were no changes
        assert!(hash.is_none(), "Should return None for no changes");

        // Verify we still have only 1 change
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn test_file_write_extends_file() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create initial file
        backend
            .record_file_create("test.txt", 0, b"Hello", "Create test.txt")
            .unwrap();

        // Write beyond the current file size
        backend
            .record_file_write("test.txt", 10, b"World", "Extend test.txt")
            .unwrap();

        // Verify content (should have zeros between)
        let retrieved = backend.get_file_content("test.txt").unwrap();
        assert_eq!(retrieved.len(), 15);
        assert_eq!(&retrieved[0..5], b"Hello");
        assert_eq!(&retrieved[5..10], &[0u8; 5]); // Zeros
        assert_eq!(&retrieved[10..15], b"World");
    }

    #[test]
    fn test_sequential_file_operations() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();

        // Create a file
        backend
            .record_file_create("test.txt", 0, b"Initial", "Create test.txt")
            .unwrap();

        // Verify we have 1 change
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 1);

        // Verify content
        let content = backend.get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Initial");

        // Modify the file
        backend
            .record_file_write("test.txt", 0, b"Modified", "Update test.txt")
            .unwrap();

        // Verify change count
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 2);

        // Verify modified content
        let content = backend.get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Modified");

        // Truncate the file
        backend
            .record_file_truncate("test.txt", 3, "Truncate test.txt")
            .unwrap();

        // Verify change count
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 3);

        // Verify truncated content
        let content = backend.get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Mod");
    }
}
