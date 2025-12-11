//! Pijul backend for Ize
//!
//! This module provides a clean interface to libpijul, handling the
//! differences between standard pijul directory structure and Ize's
//! custom layout.
//!
//! Note: This is the first backend implementation. The architecture is
//! designed to support pluggable backends in the future via a VcsBackend trait.
//!
//! ## Modules
//!
//! - [`operations`]: Opcode recording - converts filesystem operations into Pijul changes

pub mod operations;

use std::path::{Path, PathBuf};

use libpijul::pristine::sanakirja::{MutTxn, Pristine, SanakirjaError, Txn};
use libpijul::working_copy::filesystem::FileSystem as WorkingCopy;
use libpijul::{ChannelTxnT, MutTxnT, TxnT};
use thiserror::Error;

// Re-export key types from operations module
pub use operations::{OpcodeError, OpcodeRecordingBackend};

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

    // === Utility Functions ===

    /// Get the maximum number of files to keep open (for change store)
    #[allow(dead_code)]
    fn max_files() -> Result<usize, PijulError> {
        #[cfg(unix)]
        {
            if let Ok((n, _)) = rlimit::getrlimit(rlimit::Resource::NOFILE) {
                let parallelism = std::thread::available_parallelism()
                    .map(|p| p.get())
                    .unwrap_or(1);
                Ok((n as usize / (2 * parallelism)).max(1))
            } else {
                Ok(256)
            }
        }
        #[cfg(not(unix))]
        {
            Ok(1)
        }
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
}
