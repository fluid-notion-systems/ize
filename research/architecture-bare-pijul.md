# Ize Architecture: Bare Pijul

Ize transparently versions file operations into a bare Pijul repository. The existing `PassthroughFS` passes through to `working/` for immediate file access, while `OpQueue` processes changes directly against the bare `.pijul/` database via libpijul.

## Configuration

```
~/.config/ize/config.toml
```

```toml
central-dir = "~/.local/share/ize"
```

## Directory Structure

```
~/.local/share/ize/
├── config.toml
└── projects/
    └── {project-uuid}/
        ├── .pijul/              # Pijul repository (source of truth)
        │   ├── pristine/        # Sanakirja database - file content graph
        │   ├── changes/         # Patch storage
        │   └── config
        ├── working/             # User-visible state (latest committed)
        └── meta/
            └── project.toml
```

## Core Concept

**Two parallel paths:**

1. **PassthroughFS → working/** - Immediate file I/O for user
2. **OpQueue → bare .pijul/** - Async versioning via libpijul

```
User Write                                     
    │                                          
    ▼                                          
┌─────────────────┐                            
│      FUSE       │                            
│  PassthroughFS  │                            
└────────┬────────┘                            
         │                                     
         ├──────────────────┐                  
         │                  │                  
         ▼                  ▼                  
   ┌──────────┐      ┌────────────┐            
   │ working/ │      │  Enqueue   │            
   │  (write) │      │ Op + data  │            
   └──────────┘      └─────┬──────┘            
                           │                   
                           ▼                   
                     ┌────────────┐     ┌──────────┐
                     │  Process:  │     │  bare    │
                     │  libpijul  │────►│ .pijul/  │
                     │  (no FS)   │     │ pristine │
                     └────────────┘     └──────────┘
```

**Key insight:** OpQueue never touches `working/`. It operates entirely against the bare `.pijul/` directory using libpijul APIs. The `working/` directory is solely managed by PassthroughFS.

## Operation Queue

Each op is **self-contained** with full data payload.

```rust
pub enum OpType {
    Create { data: Vec<u8>, mode: u32 },
    Write { offset: i64, data: Vec<u8> },
    Unlink,
    Rename { from: String, to: String },
    Truncate { size: u64 },
    SetAttr { 
        mode: Option<u32>, 
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<u64>,
        mtime: Option<u64>,
    },
    MkDir { mode: u32 },
    RmDir,
}

pub struct Op {
    id: u64,
    op_type: OpType,
    path: String,
    timestamp: u64,
}
```

**Why data in op?** By the time we process op N, the user may have enqueued ops N+1, N+2... We cannot read from working/ as it would reflect later state.

## Pijul Direct Integration

### Reading File Content from Pristine

```rust
use libpijul::{MutTxnTExt, TxnTExt, pristine::*, output::*};

impl PijulBackend {
    /// Get file contents at HEAD (or specific change)
    fn get_file_content(&self, path: &str) -> Result<Vec<u8>> {
        let txn = self.repo.pristine.txn_begin()?;
        let channel = txn.load_channel(&self.channel_name)?
            .ok_or(Error::NoChannel)?;
        
        // Get the inode for this path
        let inode = txn.find_inode(path)?;
        
        // Output file content to memory buffer
        let mut buffer = Vec::new();
        output_file(&txn, &channel, &mut buffer, &inode)?;
        
        Ok(buffer)
    }
}
```

### Creating Changes Programmatically

```rust
impl PijulBackend {
    /// Apply an operation and create a change
    fn apply_op(&mut self, op: &Op) -> Result<Hash> {
        let mut txn = self.repo.pristine.mut_txn_begin()?;
        let channel = txn.load_channel(&self.channel_name)?
            .ok_or(Error::NoChannel)?;
        
        match &op.op_type {
            OpType::Write { offset, data } => {
                // 1. Get current content from pristine
                let mut content = self.get_file_content(&op.path)?;
                
                // 2. Apply write in memory
                let offset = *offset as usize;
                let end = offset + data.len();
                if end > content.len() {
                    content.resize(end, 0);
                }
                content[offset..end].copy_from_slice(data);
                
                // 3. Create change representing this modification
                let change = self.create_file_change(&txn, &channel, &op.path, &content)?;
                
                // 4. Apply change to pristine
                let hash = self.repo.changes.save_change(&change)?;
                txn.apply_change(&channel, &hash)?;
                
                txn.commit()?;
                Ok(hash)
            }
            OpType::Create { data, mode } => {
                // Create new file change
                let change = self.create_new_file_change(&txn, &op.path, data, *mode)?;
                let hash = self.repo.changes.save_change(&change)?;
                txn.apply_change(&channel, &hash)?;
                txn.commit()?;
                Ok(hash)
            }
            // ... other op types
        }
    }
}
```

**Note:** No sync back to working/ needed. PassthroughFS already wrote there. OpQueue just records the change in Pijul history.

## FUSE Layer (PassthroughFS)

The existing `PassthroughFS` (in `crates/ize-lib/src/filesystems/passthrough.rs`) handles all file I/O. It uses `PathManager` for inode↔path mapping and passes operations through to the underlying `working/` directory.

### Current PassthroughFS Structure

```rust
pub struct PassthroughFS {
    db_path: PathBuf,          // Legacy: was for SQLite, repurpose for .pijul/
    mount_point: PathBuf,      // Where FUSE mounts
    read_only: bool,
    path_manager: PathManager, // Handles inode ↔ path mapping
}
```

### Integration: VersionedFS Wrapper

Wrap `PassthroughFS` to add OpQueue integration:

```rust
pub struct VersionedFS {
    passthrough: PassthroughFS,
    pijul_dir: PathBuf,        // Bare .pijul/ directory
    op_queue: OpQueue,
}

impl VersionedFS {
    pub fn new(working_dir: PathBuf, pijul_dir: PathBuf, mount_point: PathBuf) -> Result<Self> {
        // PassthroughFS uses db_path's parent as source dir
        // So we pass working_dir with a dummy file to set source correctly
        let dummy_db = working_dir.join(".ize-marker");
        let passthrough = PassthroughFS::new(&dummy_db, &mount_point)?;
        
        Ok(Self {
            passthrough,
            pijul_dir,
            op_queue: OpQueue::new(),
        })
    }
}

impl Filesystem for VersionedFS {
    fn write(&mut self, req: &Request, ino: u64, fh: u64,
             offset: i64, data: &[u8], write_flags: u32, 
             flags: i32, lock_owner: Option<u64>, reply: ReplyWrite) {
        
        // Get path before passthrough (need it for op)
        let path = self.passthrough.path_manager.get_path(ino)
            .map(|p| p.to_string_lossy().to_string());
        
        // 1. Enqueue op with full data FIRST
        if let Some(path_str) = path {
            let op = Op {
                id: self.op_queue.next_id(),
                op_type: OpType::Write {
                    offset,
                    data: data.to_vec(),
                },
                path: path_str,
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };
            self.op_queue.push(op);
        }
        
        // 2. Delegate to PassthroughFS for actual write
        self.passthrough.write(req, ino, fh, offset, data, 
                               write_flags, flags, lock_owner, reply);
    }
    
    fn create(&mut self, req: &Request, parent: u64, name: &OsStr,
              mode: u32, flags: u32, umask: i32, reply: ReplyCreate) {
        // For create, we need to capture data after the file is created
        // Option: read back the file, or track via write ops
        // For now, create with empty data, writes will follow
        
        let path = self.passthrough.path_manager.build_path(parent, Path::new(name))
            .map(|p| p.to_string_lossy().to_string());
        
        if let Some(path_str) = path {
            let op = Op {
                id: self.op_queue.next_id(),
                op_type: OpType::Create {
                    data: Vec::new(),  // Content comes via subsequent writes
                    mode,
                },
                path: path_str,
                timestamp: now(),
            };
            self.op_queue.push(op);
        }
        
        self.passthrough.create(req, parent, name, mode, flags, umask, reply);
    }
    
    // Delegate read-only operations directly
    fn read(&mut self, req: &Request, ino: u64, fh: u64,
            offset: i64, size: u32, flags: i32, 
            lock_owner: Option<u64>, reply: ReplyData) {
        self.passthrough.read(req, ino, fh, offset, size, flags, lock_owner, reply);
    }
    
    // ... other operations follow same pattern:
    // 1. Build op with full data
    // 2. Enqueue
    // 3. Delegate to passthrough
}
```

### Operations That Need OpQueue Integration

| FUSE Op | OpType | Data Captured |
|---------|--------|---------------|
| `write` | `Write` | offset + data bytes |
| `create` | `Create` | mode (data via writes) |
| `unlink` | `Unlink` | path only |
| `rmdir` | `RmDir` | path only |
| `mkdir` | `MkDir` | mode |
| `rename` | `Rename` | from + to paths |
| `setattr` | `SetAttr` | changed attributes |

### Read-Only Operations (No OpQueue)

These delegate directly to `PassthroughFS`:
- `lookup`, `getattr`, `read`, `readdir`, `open`, `flush`, `release`

## Op Processing

Background thread processes queue against bare `.pijul/`:

```rust
pub struct OpProcessor {
    pijul: PijulBackend,
    queue: Arc<OpQueue>,
}

impl OpProcessor {
    pub fn new(pijul_dir: PathBuf, queue: Arc<OpQueue>) -> Result<Self> {
        Ok(Self {
            pijul: PijulBackend::open(pijul_dir)?,
            queue,
        })
    }
    
    pub fn run(&mut self) {
        loop {
            if let Some(op) = self.queue.pop() {
                match self.pijul.apply_op(&op) {
                    Ok(hash) => {
                        // No filesystem sync - working/ already has the data
                        log::info!("Recorded op {}: {} -> {}", op.id, op.path, hash);
                    }
                    Err(e) => {
                        log::error!("Failed to record op {}: {}", op.id, e);
                        // TODO: retry logic or dead-letter queue
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    
    /// Spawn processor in background thread
    pub fn spawn(pijul_dir: PathBuf, queue: Arc<OpQueue>) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut processor = OpProcessor::new(pijul_dir, queue)
                .expect("Failed to create OpProcessor");
            processor.run();
        })
    }
}
```

**Key point:** `OpProcessor` never touches `working/`. It only updates the bare `.pijul/` pristine database. The `PassthroughFS` already wrote the actual file data.

## Consistency Model

| Location | Contents | Updated By |
|----------|----------|------------|
| working/ | Current file state | PassthroughFS (immediate) |
| .pijul/pristine | Versioned history | OpQueue (async) |
| OpQueue | Pending ops + data | FUSE write ops |

**Read path**: PassthroughFS → working/ (always current)

**Write path**: 
1. PassthroughFS writes to working/ (immediate)
2. Enqueue op with data copy
3. Return to user (non-blocking)
4. Background: OpQueue applies to bare .pijul/

**Consistency**: `working/` is always ahead of or equal to `.pijul/` history. The OpQueue eventually catches up. No sync-back needed since PassthroughFS already wrote the data.

## CLI

```bash
ize init <name>                    # Create new project
ize mount <name> <mountpoint>      # Mount project
ize unmount <name>                 # Unmount
ize list                           # List projects
ize history <name> [path]          # Show changes (via pijul log)
ize restore <name> <path> <hash>   # Restore file from change
ize status <name>                  # Show pending ops
```

## libpijul Key APIs

| API | Purpose |
|-----|---------|
| `txn_begin()` | Start read transaction |
| `mut_txn_begin()` | Start write transaction |
| `load_channel()` | Get branch/channel reference |
| `find_inode()` | Path → internal inode |
| `output_file()` | Render file content from graph |
| `apply_change()` | Apply a change to channel |
| `changes.save_change()` | Persist change to disk |

## Future Considerations

- **Op persistence**: Write queue to disk for crash recovery
- **Batching**: Coalesce rapid writes before commit
- **In-memory pristine**: For tests / embedded use
- **Multiple channels**: Branch support via separate channels
- **Content hashing**: Dedup identical data in OpQueue
- **PathManager refactor**: Currently tied to `db_path` parent; could simplify for `working/` dir
- **Restore operation**: Use `output_file()` to reconstruct file from pristine, write to `working/`
