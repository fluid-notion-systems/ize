# Ize Architecture: Bare Pijul

Ize transparently versions file operations into a bare Pijul repository. The `ObservingFS<PassthroughFS>` handles immediate file I/O to `working/`, while `OpcodeRecorder` captures operations and the processor applies them to the bare `.pijul/` database via libpijul.

## Directory Structure

```
~/.local/share/ize/
├── config.toml
└── projects/
    └── {project-uuid}/
        ├── .pijul/              # Bare Pijul repository (source of truth)
        │   ├── pristine/        # Sanakirja database - file content graph
        │   ├── changes/         # Patch storage
        │   └── config
        ├── working/             # User-visible state (current filesystem)
        └── meta/
            └── project.toml
```

## Core Architecture

**Two parallel paths:**

1. **ObservingFS → PassthroughFS → working/** - Immediate file I/O for user
2. **OpcodeRecorder → OpcodeQueue → Processor → bare .pijul/** - Async versioning via libpijul

```
User Write
    │
    ▼
┌─────────────────────────────────────┐
│     ObservingFS<PassthroughFS>      │
│                                     │
│  ┌─────────────┐  ┌──────────────┐  │
│  │ FsObserver  │  │ PassthroughFS│  │
│  │ notifications│  │  (working/)  │  │
│  └──────┬──────┘  └──────┬───────┘  │
└─────────┼────────────────┼──────────┘
          │                │
          ▼                ▼
   ┌──────────────┐   ┌──────────┐
   │OpcodeRecorder│   │ working/ │
   │              │   │  (write) │
   └──────┬───────┘   └──────────┘
          │
          ▼
   ┌──────────────┐
   │ OpcodeQueue  │
   │ (VecDeque)   │
   └──────┬───────┘
          │
          ▼
   ┌──────────────┐     ┌──────────┐
   │  Processor:  │     │  bare    │
   │  libpijul    │────►│ .pijul/  │
   │  (Memory WC) │     │ pristine │
   └──────────────┘     └──────────┘
```

**Key insight:** The processor never touches `working/`. It operates entirely against the bare `.pijul/` directory using libpijul's in-memory working copy. The `working/` directory is solely managed by PassthroughFS.

## Current Implementation

### Opcode Types (implemented)

```rust
pub struct Opcode {
    seq: u64,           // Monotonic sequence number
    timestamp: u64,     // Nanoseconds since Unix epoch
    op: Operation,      // The operation
}

pub enum Operation {
    // File operations
    FileCreate { path: PathBuf, mode: u32, content: Vec<u8> },
    FileWrite { path: PathBuf, offset: u64, data: Vec<u8> },
    FileTruncate { path: PathBuf, new_size: u64 },
    FileDelete { path: PathBuf },
    FileRename { old_path: PathBuf, new_path: PathBuf },

    // Directory operations
    DirCreate { path: PathBuf, mode: u32 },
    DirDelete { path: PathBuf },
    DirRename { old_path: PathBuf, new_path: PathBuf },

    // Metadata operations
    SetPermissions { path: PathBuf, mode: u32 },
    SetTimestamps { path: PathBuf, atime: Option<u64>, mtime: Option<u64> },
    SetOwnership { path: PathBuf, uid: Option<u32>, gid: Option<u32> },

    // Link operations
    SymlinkCreate { path: PathBuf, target: PathBuf },
    SymlinkDelete { path: PathBuf },
    HardLinkCreate { existing_path: PathBuf, new_path: PathBuf },
}
```

### OpcodeQueue (implemented)

```rust
pub struct OpcodeQueue {
    inner: Mutex<QueueInner>,   // VecDeque storage
    not_empty: Condvar,          // Wake-on-push notification
}

// Key methods:
queue.try_push(opcode)  // Non-blocking, returns Err if at capacity
queue.push(opcode)      // Always succeeds (allows overflow)
queue.pop()             // Blocking - waits for item
queue.try_pop()         // Non-blocking
queue.drain()           // Get all pending opcodes
```

### OpcodeRecorder (implemented)

```rust
pub struct OpcodeRecorder {
    inode_map: InodeMap,       // Shared with PassthroughFS
    source_dir: PathBuf,       // For metadata lookups
    next_seq: AtomicU64,       // Sequence generator
    sender: OpcodeSender,      // Queue handle
}

impl FsObserver for OpcodeRecorder {
    fn on_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]) { ... }
    fn on_create(&self, parent: u64, name: &OsStr, mode: u32, ...) { ... }
    fn on_unlink(&self, parent: u64, name: &OsStr) { ... }
    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32, ...) { ... }
    fn on_rmdir(&self, parent: u64, name: &OsStr) { ... }
    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) { ... }
    fn on_setattr(&self, ino: u64, size: Option<u64>, mode: Option<u32>, ...) { ... }
    fn on_symlink(&self, parent: u64, name: &OsStr, target: &Path) { ... }
    fn on_link(&self, ino: u64, newparent: u64, newname: &OsStr) { ... }
}
```

## Pijul Integration (next phase)

### The Challenge

By the time the processor handles opcode N, the user may have already performed operations N+1, N+2, etc. We **cannot** read from `working/` as it would reflect a later state.

### Solution: Virtual Working Copy Per-Operation

Use libpijul's `Memory` type to create an in-memory working copy representing the state at the time of the operation:

```
Opcode Processing Flow:

  Opcode { path: "foo.txt", op: FileWrite { offset: 0, data: "new" } }
                     │
                     ▼
  ┌─────────────────────────────────────────┐
  │ 1. Get current content from pristine    │
  │    at channel HEAD                      │
  │    (output_file → Vec<u8>)              │
  └──────────────────┬──────────────────────┘
                     │
                     ▼
  ┌─────────────────────────────────────────┐
  │ 2. Apply op to get "new" content        │
  │    old_content[offset..] = data         │
  └──────────────────┬──────────────────────┘
                     │
                     ▼
  ┌─────────────────────────────────────────┐
  │ 3. Create Memory working copy with      │
  │    the "new" content                    │
  └──────────────────┬──────────────────────┘
                     │
                     ▼
  ┌─────────────────────────────────────────┐
  │ 4. Record change (diff pristine vs      │
  │    Memory working copy)                 │
  └──────────────────┬──────────────────────┘
                     │
                     ▼
  ┌─────────────────────────────────────────┐
  │ 5. Apply change to pristine             │
  └─────────────────────────────────────────┘
```

### libpijul Key Types

| Type | Purpose |
|------|---------|
| `Pristine` | Sanakirja database handle |
| `ArcTxn<T>` | Thread-safe transaction wrapper |
| `ChannelRef<T>` | Reference to a branch/channel |
| `Memory` | In-memory working copy |
| `ChangeStore` | Trait for storing changes |
| `RecordBuilder` | Builder for recording changes |
| `Hash` | Change identifier |

### libpijul Key Operations

| Operation | Function |
|-----------|----------|
| Begin transaction | `pristine.arc_txn_begin()` |
| Load channel | `txn.read().load_channel(name)` |
| Track file | `txn.write().add_file(path, mode)` |
| Record change | `builder.record(...)` + `builder.finish()` |
| Save change | `changes.save_change(&mut change, ...)` |
| Apply locally | `apply_local_change(...)` |
| Output file | `output::output_file(...)` |
| Commit transaction | `txn.commit()` |

### PijulBackend Sketch

```rust
use libpijul::working_copy::memory::Memory;
use libpijul::pristine::sanakirja::Pristine;
use libpijul::changestore::filesystem::FileSystem as ChangeStore;

pub struct PijulBackend {
    pristine: Pristine,
    changes: ChangeStore,
    channel_name: String,
}

impl PijulBackend {
    /// Apply an opcode and create a change
    pub fn apply_opcode(&self, opcode: &Opcode) -> Result<Option<Hash>, Error> {
        let txn = self.pristine.arc_txn_begin()?;
        let channel = {
            let t = txn.read();
            t.load_channel(&self.channel_name)?.unwrap()
        };

        match opcode.op() {
            Operation::FileWrite { path, offset, data } => {
                // 1. Get current file content from pristine
                let current_content = self.get_file_content(&txn, &channel, path)?;

                // 2. Apply the write operation in memory
                let mut new_content = current_content;
                let offset = *offset as usize;
                let end = offset + data.len();
                if end > new_content.len() {
                    new_content.resize(end, 0);
                }
                new_content[offset..end].copy_from_slice(data);

                // 3. Create in-memory working copy with new content
                let memory_wc = Memory::new();
                memory_wc.add_file(path.to_str().unwrap(), new_content);

                // 4. Record the change
                let mut builder = libpijul::RecordBuilder::new();
                builder.record(
                    txn.clone(),
                    libpijul::Algorithm::default(),
                    false,
                    &libpijul::DEFAULT_SEPARATOR,
                    channel.clone(),
                    &memory_wc,
                    &self.changes,
                    path.to_str().unwrap(),
                    1,
                )?;

                let rec = builder.finish();
                if rec.actions.is_empty() {
                    return Ok(None); // No changes
                }

                // 5. Create and save the change
                let changes = rec.actions
                    .into_iter()
                    .map(|r| r.globalize(&*txn.read()).unwrap())
                    .collect();

                let mut change = libpijul::change::Change::make_change(
                    &*txn.read(),
                    &channel,
                    changes,
                    std::mem::take(&mut *rec.contents.lock()),
                    libpijul::change::ChangeHeader {
                        message: format!("write to {}", path.display()),
                        authors: vec![],
                        description: None,
                        timestamp: jiff::Timestamp::from_nanosecond(opcode.timestamp() as i128)?,
                    },
                    Vec::new(),
                )?;

                let hash = self.changes.save_change(&mut change, |_, _| Ok::<_, Error>(()))?;

                // 6. Apply to local pristine
                libpijul::apply::apply_local_change(
                    &mut *txn.write(),
                    &channel,
                    &change,
                    &hash,
                    &rec.updatables,
                )?;

                txn.commit()?;
                Ok(Some(hash))
            }

            Operation::FileCreate { path, mode, content } => {
                let memory_wc = Memory::new();
                memory_wc.add_file(path.to_str().unwrap(), content.clone());
                // ... record and apply similar to FileWrite
            }

            Operation::FileDelete { path } => {
                // Record deletion via Memory working copy without the file
                // ...
            }

            // ... other operation types
        }
    }

    /// Get file content from pristine at HEAD
    fn get_file_content(
        &self,
        txn: &ArcTxn<impl libpijul::MutTxnT>,
        channel: &ChannelRef<impl libpijul::MutTxnT>,
        path: &PathBuf,
    ) -> Result<Vec<u8>, Error> {
        let t = txn.read();
        let c = channel.read();

        let (pos, _) = t.follow_oldest_path(&self.changes, &*c, path.to_str().unwrap())?;

        let mut buffer = Vec::new();
        libpijul::output::output_file(
            &self.changes,
            txn,
            channel,
            pos,
            &mut libpijul::vertex_buffer::Writer::new(&mut buffer),
        )?;

        Ok(buffer)
    }
}
```

### Opcode Processor

```rust
pub struct OpcodeProcessor {
    pijul: PijulBackend,
    queue: Arc<OpcodeQueue>,
}

impl OpcodeProcessor {
    pub fn run(&mut self) {
        loop {
            let opcode = self.queue.pop(); // Blocks until available

            match self.pijul.apply_opcode(&opcode) {
                Ok(Some(hash)) => {
                    log::info!("Recorded opcode #{}: {} -> {}",
                        opcode.seq(), opcode.path().display(), hash);
                }
                Ok(None) => {
                    log::debug!("Opcode #{} resulted in no change", opcode.seq());
                }
                Err(e) => {
                    log::error!("Failed to process opcode #{}: {}", opcode.seq(), e);
                    // TODO: retry logic or dead-letter queue
                }
            }
        }
    }

    pub fn spawn(pijul_dir: PathBuf, queue: Arc<OpcodeQueue>) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut processor = OpcodeProcessor::new(pijul_dir, queue)
                .expect("Failed to create OpcodeProcessor");
            processor.run();
        })
    }
}
```

## Consistency Model

| Location | Contents | Updated By |
|----------|----------|------------|
| `working/` | Current file state | PassthroughFS (immediate) |
| `.pijul/pristine` | Versioned history | OpcodeProcessor (async) |
| `OpcodeQueue` | Pending ops + data | OpcodeRecorder |

**Read path**: PassthroughFS → `working/` (always current)

**Write path**:
1. PassthroughFS writes to `working/` (immediate)
2. OpcodeRecorder captures operation with data copy
3. Return to user (non-blocking)
4. Background: OpcodeProcessor applies to bare `.pijul/`

**Consistency**: `working/` is always ahead of or equal to `.pijul/` history. The OpcodeProcessor eventually catches up. No sync-back needed since PassthroughFS already wrote the data.

## Performance Considerations

### Reading from Pristine

`output_file()` reconstructs a file by traversing the graph. For small-to-medium files, this is fast. For very large files with many changes, it may become slower.

**Mitigation:** Cache recently-read file contents keyed by (path, channel_state).

### Recording Changes

The `record` operation diffs the working copy against the pristine. With an in-memory working copy containing only the changed file(s), this should be fast.

**Optimization:** Use `prefix` parameter to scope recording to just the affected file.

### Coalescing Operations

Multiple rapid writes to the same file could be coalesced:
- Buffer ops for the same file within a time window
- Apply all buffered ops to get final content
- Record single change

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

## Future Considerations

- **Op persistence**: Write queue to disk for crash recovery
- **Batching**: Coalesce rapid writes before commit
- **In-memory pristine**: For tests / embedded use
- **Multiple channels**: Branch support via separate channels
- **Content hashing**: Dedup identical data in OpcodeQueue
- **Restore operation**: Use `output_file()` to reconstruct file from pristine, write to `working/`
