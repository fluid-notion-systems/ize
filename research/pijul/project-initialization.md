# Project Initialization Research

## Overview

This document covers the initialization of an Ize project, including:
1. Central storage location for projects
2. Directory structure creation
3. Programmatic initialization of a bare `.pijul/` repository
4. Creating the initial channel
5. **PijulBackend** - A backend implementation for pijul version control

> **Note: Pluggable Backend Architecture**
> 
> The `PijulBackend` is the first implementation of Ize's version control backend.
> The architecture is designed to be **pluggable**, allowing future backends to be
> implemented (e.g., Git, SQLite-based, or custom storage engines). All backends
> will implement a common `VcsBackend` trait, enabling users to choose their
> preferred version control system while maintaining the same Ize workflow.

## Pijul Source Code Analysis

### How `pijul init` Works

From analyzing `vendor/pijul/pijul/src/commands/init.rs`:

```rust
impl Init {
    pub fn run(self) -> Result<(), anyhow::Error> {
        let repo = Repository::init(self.path.as_deref(), self.kind.as_deref(), None)?;
        let mut txn = repo.pristine.mut_txn_begin()?;
        let channel_name = self
            .channel
            .unwrap_or_else(|| libpijul::DEFAULT_CHANNEL.to_string());
        txn.open_or_create_channel(&channel_name)?;
        txn.set_current_channel(&channel_name)?;
        txn.commit()?;
        Ok(())
    }
}
```

### Repository Structure from `pijul-repository`

From `vendor/pijul/pijul-repository/src/lib.rs`:

```rust
pub const PRISTINE_DIR: &str = "pristine";
pub const CHANGES_DIR: &str = "changes";
pub const CONFIG_FILE: &str = "config";

pub struct Repository {
    pub pristine: libpijul::pristine::sanakirja::Pristine,
    pub changes: libpijul::changestore::filesystem::FileSystem,
    pub working_copy: libpijul::working_copy::filesystem::FileSystem,
    pub config: config::Config,
    pub path: PathBuf,
    pub changes_dir: PathBuf,
}
```

### Pristine Database Initialization

From `vendor/pijul/libpijul/src/pristine/sanakirja.rs`:

```rust
impl Pristine {
    pub fn new<P: AsRef<Path>>(name: P) -> Result<Self, SanakirjaError> {
        Self::new_with_size(name, 1 << 20)  // 1MB initial size
    }
    
    pub fn new_with_size<P: AsRef<Path>>(name: P, size: u64) -> Result<Self, SanakirjaError> {
        let env = ::sanakirja::Env::new(name, size, 2);
        // ... error handling ...
        Ok(Pristine { env: Arc::new(env) })
    }
}
```

### Key Insight: Directory Structure Difference

**Pijul standard:** `.pijul/` lives inside the working directory (parent of `.pijul/` is the working copy)

**Ize:** `.pijul/` and `working/` are siblings under `~/.local/share/ize/projects/{uuid}/`

This means we **cannot directly use `Repository::init()`** but must use the lower-level primitives.

## Central Storage Location

### Configuration

```
~/.config/ize/config.toml
```

```toml
# Central directory for all Ize project data
central-dir = "~/.local/share/ize"
```

### Directory Structure

```
~/.local/share/ize/
├── config.toml              # Global Ize configuration
└── projects/
    └── {project-uuid}/      # Each project gets a UUID
        ├── .pijul/          # Bare Pijul repository
        │   ├── pristine/    # Sanakirja database
        │   │   └── db       # The actual database file
        │   ├── changes/     # Change files
        │   └── config       # Pijul config (hooks, etc.)
        ├── working/         # User-visible filesystem (PassthroughFS target)
        └── meta/
            └── project.toml # Project metadata (name, created, etc.)
```

### Why UUIDs?

- **Uniqueness:** No collisions even if user creates projects with same name
- **Portability:** Can rename project without breaking internal references
- **Simplicity:** No need to sanitize/escape project names for filesystem

### Project Metadata (`meta/project.toml`)

```toml
[project]
name = "my-project"
uuid = "550e8400-e29b-41d4-a716-446655440000"
created = "2024-01-15T10:30:00Z"

[pijul]
default_channel = "main"
```

## PijulBackend

The `PijulBackend` is Ize's implementation of version control using [Pijul](https://pijul.org/), a patch-based distributed version control system. It encapsulates all pijul/libpijul interactions, providing a clean API for Ize.

> **Future: VcsBackend Trait**
> 
> In the future, `PijulBackend` will implement a `VcsBackend` trait that defines
> the common interface for all version control backends:
> 
> ```rust
> pub trait VcsBackend: Send + Sync {
>     fn init(backend_dir: &Path, working_dir: &Path) -> Result<Self, BackendError>;
>     fn open(backend_dir: &Path, working_dir: &Path) -> Result<Self, BackendError>;
>     fn record_change(&self, description: &str) -> Result<ChangeId, BackendError>;
>     fn list_changes(&self) -> Result<Vec<ChangeInfo>, BackendError>;
>     fn create_branch(&self, name: &str) -> Result<(), BackendError>;
>     fn switch_branch(&mut self, name: &str) -> Result<(), BackendError>;
>     fn list_branches(&self) -> Result<Vec<String>, BackendError>;
>     // ... additional methods
> }
> ```
> 
> This will allow Ize to support multiple VCS backends (Git, custom, etc.) in the future.

### Design Goals

1. **Encapsulation:** Hide libpijul complexity behind a simple interface
2. **Ize-specific:** Handle our custom directory structure (separate `.pijul/` and `working/`)
3. **Error handling:** Convert pijul errors to Ize errors
4. **Thread safety:** Support concurrent access where needed
5. **Trait-ready:** Designed to eventually implement a `VcsBackend` trait

### Implementation

```rust
//! Pijul backend for Ize
//! 
//! This module provides a clean interface to libpijul, handling the
//! differences between standard pijul directory structure and Ize's
//! custom layout.
//!
//! Note: This is the first backend implementation. The architecture is
//! designed to support pluggable backends in the future via a VcsBackend trait.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libpijul::pristine::sanakirja::{Pristine, SanakirjaError, MutTxn, Txn};
use libpijul::changestore::filesystem::FileSystem as ChangeStore;
use libpijul::{MutTxnT, MutTxnTExt, TxnT, TxnTExt, ChannelTxnT, ChannelRef, Hash};
use libpijul::working_copy::filesystem::FileSystem as WorkingCopy;
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
                .unwrap_or_else(|| libpijul::DEFAULT_CHANNEL.to_string())
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
    
    /// Create a new change store for this repository
    pub fn change_store(&self) -> Result<ChangeStore, PijulError> {
        // Note: FileSystem::from_root expects the repository root, not changes dir
        // It will look for changes in {root}/.pijul/changes/
        // Since our structure is different, we need to handle this carefully
        let max_files = Self::max_files()?;
        Ok(ChangeStore::from_root(&self.pijul_dir.parent().unwrap_or(&self.pijul_dir), max_files))
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
        if txn.load_channel(name)?.is_none() {
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
        
        for channel in txn.iter_channels("")? {
            let channel = channel.map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
            channels.push(txn.name(&channel).to_string());
        }
        
        Ok(channels)
    }
    
    /// Fork a channel (create a new channel from an existing one)
    pub fn fork_channel(&self, from: &str, to: &str) -> Result<(), PijulError> {
        let mut txn = self.pristine.mut_txn_begin()?;
        
        let from_channel = txn.load_channel(from)?
            .ok_or_else(|| PijulError::ChannelNotFound(from.to_string()))?;
        
        txn.fork(&from_channel, to)?;
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
        let wrapper = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        assert_eq!(wrapper.current_channel(), "main");
        assert!(pijul_dir.join("pristine/db").exists());
        assert!(pijul_dir.join("changes").exists());
        assert!(pijul_dir.join("config").exists());
        
        // Open existing
        drop(wrapper);
        let wrapper = PijulBackend::open(&pijul_dir, &working_dir).unwrap();
        assert_eq!(wrapper.current_channel(), "main");
    }
    
    #[test]
    fn test_custom_channel() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");
        
        let wrapper = PijulBackend::init(&pijul_dir, &working_dir, Some("dev")).unwrap();
        assert_eq!(wrapper.current_channel(), "dev");
        
        let channels = wrapper.list_channels().unwrap();
        assert!(channels.contains(&"dev".to_string()));
    }
    
    #[test]
    fn test_channel_operations() {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");
        
        let mut wrapper = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        
        // Create a new channel
        wrapper.create_channel("feature").unwrap();
        
        // List channels
        let channels = wrapper.list_channels().unwrap();
        assert!(channels.contains(&"main".to_string()));
        assert!(channels.contains(&"feature".to_string()));
        
        // Switch channel
        wrapper.switch_channel("feature").unwrap();
        assert_eq!(wrapper.current_channel(), "feature");
        
        // Switch to non-existent channel should fail
        assert!(wrapper.switch_channel("nonexistent").is_err());
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
```

## IzeProject with PijulBackend

Updated implementation using the PijulBackend:

```rust
use std::path::{Path, PathBuf};

pub struct IzeProject {
    pub project_dir: PathBuf,
    pub pijul: PijulBackend,
    pub meta_dir: PathBuf,
}

impl IzeProject {
    /// Initialize a new Ize project
    pub fn init(project_dir: &Path, name: &str) -> Result<Self, Error> {
        let pijul_dir = project_dir.join(".pijul");
        let working_dir = project_dir.join("working");
        let meta_dir = project_dir.join("meta");
        
        // Create meta directory
        std::fs::create_dir_all(&meta_dir)?;
        
        // Initialize pijul via wrapper
        let pijul = PijulBackend::init(&pijul_dir, &working_dir, None)?;
        
        // Write project metadata
        let uuid = uuid::Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();
        let meta_toml = format!(
            "[project]\nname = {:?}\nuuid = {:?}\ncreated = {:?}\n\n[pijul]\ndefault_channel = {:?}\n",
            name, uuid.to_string(), now, pijul.current_channel()
        );
        std::fs::write(meta_dir.join("project.toml"), meta_toml)?;
        
        Ok(Self {
            project_dir: project_dir.to_path_buf(),
            pijul,
            meta_dir,
        })
    }
    
    /// Open an existing project
    pub fn open(project_dir: &Path) -> Result<Self, Error> {
        let pijul_dir = project_dir.join(".pijul");
        let working_dir = project_dir.join("working");
        let meta_dir = project_dir.join("meta");
        
        let pijul = PijulBackend::open(&pijul_dir, &working_dir)?;
        
        Ok(Self {
            project_dir: project_dir.to_path_buf(),
            pijul,
            meta_dir,
        })
    }
    
    /// Get the working directory path
    pub fn working_dir(&self) -> &Path {
        self.pijul.working_dir()
    }
    
    /// Get the pijul directory path  
    pub fn pijul_dir(&self) -> &Path {
        self.pijul.pijul_dir()
    }
}
```

## Project Manager

A higher-level abstraction for managing multiple projects:

```rust
pub struct ProjectManager {
    central_dir: PathBuf,
}

impl ProjectManager {
    pub fn new() -> Result<Self, Error> {
        // Default location
        let central_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ize");
        
        std::fs::create_dir_all(&central_dir)?;
        std::fs::create_dir_all(central_dir.join("projects"))?;
        
        Ok(Self { central_dir })
    }
    
    pub fn with_central_dir(central_dir: PathBuf) -> Result<Self, Error> {
        std::fs::create_dir_all(&central_dir)?;
        std::fs::create_dir_all(central_dir.join("projects"))?;
        Ok(Self { central_dir })
    }
    
    /// Create a new project
    pub fn create_project(&self, name: &str) -> Result<IzeProject, Error> {
        let uuid = uuid::Uuid::new_v4();
        let project_dir = self.central_dir.join("projects").join(uuid.to_string());
        
        IzeProject::init(&project_dir, name)
    }
    
    /// Find a project by name
    pub fn find_project(&self, name: &str) -> Result<Option<IzeProject>, Error> {
        let projects_dir = self.central_dir.join("projects");
        
        for entry in std::fs::read_dir(&projects_dir)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta").join("project.toml");
            
            if meta_path.exists() {
                let content = std::fs::read_to_string(&meta_path)?;
                if let Ok(meta) = toml::from_str::<ProjectMeta>(&content) {
                    if meta.project.name == name {
                        return Ok(Some(IzeProject::open(&entry.path())?));
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// List all projects
    pub fn list_projects(&self) -> Result<Vec<ProjectInfo>, Error> {
        let projects_dir = self.central_dir.join("projects");
        let mut projects = Vec::new();
        
        for entry in std::fs::read_dir(&projects_dir)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta").join("project.toml");
            
            if meta_path.exists() {
                let content = std::fs::read_to_string(&meta_path)?;
                if let Ok(meta) = toml::from_str::<ProjectMeta>(&content) {
                    projects.push(ProjectInfo {
                        name: meta.project.name,
                        uuid: meta.project.uuid,
                        path: entry.path(),
                    });
                }
            }
        }
        
        Ok(projects)
    }
}

#[derive(Debug, Deserialize)]
struct ProjectMeta {
    project: ProjectSection,
    pijul: PijulSection,
}

#[derive(Debug, Deserialize)]
struct ProjectSection {
    name: String,
    uuid: String,
    created: String,
}

#[derive(Debug, Deserialize)]
struct PijulSection {
    default_channel: String,
}

#[derive(Debug)]
pub struct ProjectInfo {
    pub name: String,
    pub uuid: String,
    pub path: PathBuf,
}
```

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Pijul - use path dependency for vendored version
libpijul = { path = "../vendor/pijul/libpijul", default-features = false, features = ["ondisk-repos"] }

# Or from crates.io:
# libpijul = { version = "1.0.0-beta.10", default-features = false, features = ["ondisk-repos"] }

uuid = { version = "1.0", features = ["v4"] }
chrono = "0.4"
toml = "0.8"
serde = { version = "1.0", features = ["derive"] }
dirs = "5.0"
thiserror = "1.0"
rlimit = "0.10"  # For max_files calculation on Unix
```

## CLI Commands

### `ize init <name>`

Create a new versioned project:

```rust
fn cmd_init(name: &str) -> Result<(), Error> {
    let manager = ProjectManager::new()?;
    
    // Check if project with this name already exists
    if manager.find_project(name)?.is_some() {
        return Err(Error::ProjectExists(name.to_string()));
    }
    
    let project = manager.create_project(name)?;
    
    println!("Created project '{}' at {:?}", name, project.working_dir());
    println!("Mount with: ize mount {} <mountpoint>", name);
    
    Ok(())
}
```

### `ize mount <name> <mountpoint>`

Mount an existing project:

```rust
fn cmd_mount(name: &str, mountpoint: &Path) -> Result<(), Error> {
    let manager = ProjectManager::new()?;
    
    let project = manager.find_project(name)?
        .ok_or_else(|| Error::ProjectNotFound(name.to_string()))?;
    
    // Set up the filesystem
    let queue = OpcodeQueue::new();
    let passthrough = PassthroughFS::new(project.working_dir(), mountpoint)?;
    let inode_map = passthrough.inode_map();
    
    let recorder = OpcodeRecorder::new(
        inode_map,
        project.working_dir().to_path_buf(),
        queue.sender(),
    );
    
    let mut observing_fs = ObservingFS::new(passthrough);
    observing_fs.add_observer(Arc::new(recorder));
    
    // TODO: Spawn opcode processor thread with project.pijul
    
    // Mount
    let options = vec![
        MountOption::FSName(format!("ize-{}", name)),
        MountOption::AutoUnmount,
        MountOption::AllowOther,
    ];
    
    fuser::mount2(observing_fs, mountpoint, &options)?;
    
    Ok(())
}
```

### `ize list`

List all projects:

```rust
fn cmd_list() -> Result<(), Error> {
    let manager = ProjectManager::new()?;
    let projects = manager.list_projects()?;
    
    if projects.is_empty() {
        println!("No projects found.");
        println!("Create one with: ize init <name>");
    } else {
        println!("{:<20} {:<36} {}", "NAME", "UUID", "PATH");
        println!("{}", "-".repeat(80));
        for p in projects {
            println!("{:<20} {:<36} {:?}", p.name, p.uuid, p.path);
        }
    }
    
    Ok(())
}
```

## Testing

### Unit Test: Project Initialization

```rust
#[test]
fn test_project_init() {
    let temp_dir = tempfile::tempdir().unwrap();
    let manager = ProjectManager::with_central_dir(temp_dir.path().to_path_buf()).unwrap();
    
    let project = manager.create_project("test-project").unwrap();
    
    // Verify structure
    assert!(project.pijul_dir().exists());
    assert!(project.pijul_dir().join("pristine/db").exists());
    assert!(project.pijul_dir().join("changes").exists());
    assert!(project.working_dir().exists());
    assert!(project.meta_dir.join("project.toml").exists());
    
    // Verify channel exists
    let channels = project.pijul.list_channels().unwrap();
    assert!(channels.contains(&"main".to_string()));
}
```

### Integration Test: Full Flow

```rust
#[test]
fn test_full_project_flow() {
    let temp_dir = tempfile::tempdir().unwrap();
    let manager = ProjectManager::with_central_dir(temp_dir.path().to_path_buf()).unwrap();
    
    // Create project
    let project = manager.create_project("my-app").unwrap();
    
    // Find it
    let found = manager.find_project("my-app").unwrap().unwrap();
    assert_eq!(found.pijul_dir(), project.pijul_dir());
    
    // List projects
    let list = manager.list_projects().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "my-app");
    
    // Verify pijul wrapper is functional
    let channels = project.pijul.list_channels().unwrap();
    assert!(channels.contains(&"main".to_string()));
    
    // Create a new channel
    project.pijul.create_channel("feature-branch").unwrap();
    let channels = project.pijul.list_channels().unwrap();
    assert_eq!(channels.len(), 2);
}
```

## Summary

### What Happens on `ize init <name>`

1. Generate UUID for project
2. Create directory structure:
   - `~/.local/share/ize/projects/{uuid}/`
   - `~/.local/share/ize/projects/{uuid}/.pijul/`
   - `~/.local/share/ize/projects/{uuid}/.pijul/pristine/db`
   - `~/.local/share/ize/projects/{uuid}/.pijul/changes/`
   - `~/.local/share/ize/projects/{uuid}/.pijul/config`
   - `~/.local/share/ize/projects/{uuid}/working/`
   - `~/.local/share/ize/projects/{uuid}/meta/`
3. Initialize Sanakirja pristine database via `PijulBackend::init()`
4. Create default "main" channel
5. Write `project.toml` metadata

### Key libpijul APIs (wrapped by PijulBackend)

| API | PijulBackend Method | Purpose |
|-----|---------------------|---------|
| `Pristine::new(path)` | `init()` / `open()` | Create/open pristine database |
| `pristine.mut_txn_begin()` | `mut_txn_begin()` | Start mutable transaction |
| `pristine.txn_begin()` | `txn_begin()` | Start read-only transaction |
| `pristine.arc_txn_begin()` | `arc_txn_begin()` | Start thread-safe transaction |
| `txn.open_or_create_channel(name)` | `create_channel()` | Create a channel |
| `txn.load_channel(name)` | `switch_channel()` | Load a channel |
| `txn.iter_channels("")` | `list_channels()` | List all channels |
| `txn.fork(channel, name)` | `fork_channel()` | Fork a channel |
| `txn.set_current_channel(name)` | `switch_channel()` | Set current channel |
| `txn.commit()` | (internal) | Commit transaction |

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                     IzeProject                          │
│  - project_dir: PathBuf                                 │
│  - meta_dir: PathBuf                                    │
│  - pijul: PijulBackend ─────────────────────┐           │
└─────────────────────────────────────────────│───────────┘
                                              │
┌─────────────────────────────────────────────▼───────────┐
│                    PijulBackend                         │
│  - pijul_dir: PathBuf                                   │
│  - working_dir: PathBuf                                 │
│  - pristine: Pristine                                   │
│  - current_channel: String                              │
│                                                         │
│  Methods:                                               │
│  - init() / open()                                      │
│  - create_channel() / switch_channel() / list_channels()│
│  - fork_channel()                                       │
│  - txn_begin() / mut_txn_begin() / arc_txn_begin()     │
│  - change_store() / working_copy()                      │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│                      libpijul                           │
│  - Pristine (sanakirja database)                        │
│  - ChangeStore (filesystem)                             │
│  - WorkingCopy (filesystem)                             │
│  - Transactions (Txn, MutTxn, ArcTxn)                  │
└─────────────────────────────────────────────────────────┘
```

### Next Steps

1. Implement `PijulBackend` in `ize-lib/src/pijul/mod.rs`
2. Implement `IzeProject` using `PijulBackend`
3. Implement `ProjectManager` in `ize-lib`
4. Update CLI `init` command to use new initialization
5. Update CLI `mount` command to use project paths
6. Add `list` command to CLI
7. Wire up opcode processor to use `PijulBackend` for recording changes
