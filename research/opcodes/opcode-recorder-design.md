# OpcodeRecorder Design

## Overview

The `OpcodeRecorder` is the bridge between filesystem observations and the opcode queue. It implements `FsObserver` to receive notifications from `ObservingFS`, translates inodes to paths, constructs `Opcode` instances, and enqueues them for async processing.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         ObservingFS<PassthroughFS>                   │
│                                                                      │
│  on_write(ino, fh, offset, data)                                    │
│  on_create(parent, name, mode)                                       │
│  on_unlink(parent, name)                                            │
│  ...                                                                 │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          OpcodeRecorder                              │
│                                                                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────┐  │
│  │   InodeMap      │  │  SequenceGen    │  │    OpcodeQueue      │  │
│  │ (shared w/ FS)  │  │  (AtomicU64)    │  │   (channel tx)      │  │
│  └────────┬────────┘  └────────┬────────┘  └──────────┬──────────┘  │
│           │                    │                      │              │
│           ▼                    ▼                      ▼              │
│     resolve path         next seq()            enqueue(opcode)      │
└─────────────────────────────────────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        OpcodeQueue (Receiver)                        │
│                                                                      │
│  Background thread processes opcodes → Pijul changes                 │
└─────────────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. OpcodeRecorder

```rust
pub struct OpcodeRecorder {
    /// Shared inode-to-path mapping from PassthroughFS
    inode_map: InodeMap,
    
    /// Monotonic sequence number generator
    next_seq: AtomicU64,
    
    /// Channel sender for enqueuing opcodes
    queue_tx: crossbeam_channel::Sender<Opcode>,
}
```

### 2. OpcodeQueue

```rust
pub struct OpcodeQueue {
    /// Channel receiver for dequeuing opcodes
    rx: crossbeam_channel::Receiver<Opcode>,
    
    /// Channel sender (cloned to recorders)
    tx: crossbeam_channel::Sender<Opcode>,
}
```

## Design Decisions

### Why VecDeque + Channel Notification?

The design uses two components:
1. **`VecDeque`** - Actual storage for opcodes (inspectable, persistent-ready)
2. **Channel** - Notification mechanism to wake the processor

| Approach | Pros | Cons |
|----------|------|------|
| Channel only | Simple | Can't inspect queue, no persistence |
| `Mutex<VecDeque>` only | Simple, inspectable | Processor must poll or sleep |
| **VecDeque + notify channel** | Inspectable, wake-on-push, persistence-ready | Slightly more complex |

**Recommendation:** `Arc<Mutex<VecDeque>>` with a notification channel.
- Opcodes stored in `VecDeque` (can be inspected, serialized, persisted)
- Channel just signals "new item available" (sends `()`, no serialization)
- Processor blocks on channel, then drains queue
- No Serde required for the channel - it just passes unit `()`

### Bounded vs Unbounded Queue

**Bounded (recommended):**
- Prevents unbounded memory growth
- Can implement backpressure policy (block, drop oldest, drop newest)
- Configurable max capacity

**Capacity sizing:**
- Default: 10,000 opcodes
- At ~1KB average per opcode = ~10MB buffer
- Configurable via builder

### Path Resolution Strategy

The observer receives inodes, but opcodes need paths. Options:

1. **Resolve at notification time (recommended):**
   - Observer looks up path immediately from shared `InodeMap`
   - Path captured reflects state at operation time
   - Simple, deterministic

2. **Defer to processing time:**
   - Store inode in opcode, resolve later
   - Risk: inode may be reused or path changed
   - More complex, less accurate

### Handling Path Resolution Failures

When inode lookup fails (shouldn't happen normally):

```rust
fn resolve_inode(&self, ino: u64) -> Option<PathBuf> {
    self.inode_map.read().ok()?.get(&ino).cloned()
}

fn on_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]) {
    let path = match self.resolve_inode(ino) {
        Some(p) => p,
        None => {
            log::warn!("Failed to resolve inode {} for write", ino);
            return; // Skip this opcode
        }
    };
    // ... create and enqueue opcode
}
```

**Policy:** Log and skip. Missing one opcode is better than panicking or blocking.

### Distinguishing File vs Directory Operations

Some operations (rename, unlink) need to know if target is file or directory to generate the correct opcode variant.

**Options:**

1. **Check filesystem at notification time:**
   ```rust
   fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {
       let old_path = self.resolve_with_name(parent, name)?;
       let is_dir = std::fs::metadata(&self.to_real(&old_path))
           .map(|m| m.is_dir())
           .unwrap_or(false);
       
       if is_dir {
           self.emit(Operation::DirRename { ... });
       } else {
           self.emit(Operation::FileRename { ... });
       }
   }
   ```

2. **Store file type in InodeMap:**
   ```rust
   pub type InodeMap = Arc<RwLock<HashMap<u64, (PathBuf, FileType)>>>;
   ```
   
3. **Use generic "Rename" opcode:**
   - Defer file-vs-dir distinction to processing time
   - Simpler observer, slightly more complex processor

**Recommendation:** Option 1 for now - check at notification time. It's a single metadata lookup and keeps opcodes precise.

### Symlink Detection for Unlink

`unlink()` can delete files or symlinks. To generate correct opcode:

```rust
fn on_unlink(&self, parent: u64, name: &OsStr) {
    let path = self.resolve_with_name(parent, name)?;
    let real_path = self.to_real(&path);
    
    // Use symlink_metadata to not follow symlinks
    let is_symlink = std::fs::symlink_metadata(&real_path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    
    if is_symlink {
        self.emit(Operation::SymlinkDelete { path });
    } else {
        self.emit(Operation::FileDelete { path });
    }
}
```

## Implementation

### OpcodeRecorder

```rust
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use crossbeam_channel::Sender;

use crate::filesystems::observing::FsObserver;
use crate::filesystems::passthrough::InodeMap;
use crate::operations::{Opcode, Operation};

pub struct OpcodeRecorder {
    /// Shared inode-to-path mapping
    inode_map: InodeMap,
    
    /// Path to the source directory (for metadata lookups)
    source_dir: PathBuf,
    
    /// Sequence number generator
    next_seq: AtomicU64,
    
    /// Queue sender
    tx: Sender<Opcode>,
}

impl OpcodeRecorder {
    pub fn new(inode_map: InodeMap, source_dir: PathBuf, sender: OpcodeSender) -> Self {
        Self {
            inode_map,
            source_dir,
            next_seq: AtomicU64::new(1),
            sender,
        }
    }
    
    /// Generate next sequence number
    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::SeqCst)
    }
    
    /// Resolve inode to relative path
    fn resolve_inode(&self, ino: u64) -> Option<PathBuf> {
        self.inode_map.read().ok()?.get(&ino).cloned()
    }
    
    /// Resolve parent inode + name to relative path
    fn resolve_with_name(&self, parent: u64, name: &OsStr) -> Option<PathBuf> {
        self.resolve_inode(parent).map(|p| p.join(name))
    }
    
    /// Convert relative path to real (source) path
    fn to_real(&self, rel_path: &PathBuf) -> PathBuf {
        self.source_dir.join(rel_path)
    }
    
    /// Emit an opcode to the queue
    fn emit(&self, op: Operation) {
        let opcode = Opcode::new(self.next_seq(), op);
        if let Err(_opcode) = self.sender.try_send(opcode) {
            log::warn!("Failed to enqueue opcode: queue at capacity");
            // Could implement fallback: self.sender.send(opcode) to force push
        }
    }
}

impl FsObserver for OpcodeRecorder {
    fn on_write(&self, ino: u64, _fh: u64, offset: i64, data: &[u8]) {
        let path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                log::warn!("on_write: failed to resolve inode {}", ino);
                return;
            }
        };
        
        self.emit(Operation::FileWrite {
            path,
            offset: offset as u64,
            data: data.to_vec(),
        });
    }
    
    fn on_create(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_create: failed to resolve parent {}", parent);
                return;
            }
        };
        
        self.emit(Operation::FileCreate {
            path,
            mode,
            content: Vec::new(), // Content will come via on_write
        });
    }
    
    fn on_unlink(&self, parent: u64, name: &OsStr) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_unlink: failed to resolve parent {}", parent);
                return;
            }
        };
        
        // Check if it's a symlink
        let real_path = self.to_real(&path);
        let is_symlink = std::fs::symlink_metadata(&real_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        
        if is_symlink {
            self.emit(Operation::SymlinkDelete { path });
        } else {
            self.emit(Operation::FileDelete { path });
        }
    }
    
    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_mkdir: failed to resolve parent {}", parent);
                return;
            }
        };
        
        self.emit(Operation::DirCreate { path, mode });
    }
    
    fn on_rmdir(&self, parent: u64, name: &OsStr) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_rmdir: failed to resolve parent {}", parent);
                return;
            }
        };
        
        self.emit(Operation::DirDelete { path });
    }
    
    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {
        let old_path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_rename: failed to resolve old parent {}", parent);
                return;
            }
        };
        
        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                log::warn!("on_rename: failed to resolve new parent {}", newparent);
                return;
            }
        };
        
        // Check if source is a directory
        let real_old = self.to_real(&old_path);
        let is_dir = std::fs::metadata(&real_old)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        
        if is_dir {
            self.emit(Operation::DirRename { old_path, new_path });
        } else {
            self.emit(Operation::FileRename { old_path, new_path });
        }
    }
    
    fn on_setattr(
        &self,
        ino: u64,
        size: Option<u64>,
        mode: Option<u32>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) {
        let path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                log::warn!("on_setattr: failed to resolve inode {}", ino);
                return;
            }
        };
        
        // Emit separate opcodes for each attribute change
        if let Some(new_size) = size {
            self.emit(Operation::FileTruncate {
                path: path.clone(),
                new_size,
            });
        }
        
        if let Some(new_mode) = mode {
            self.emit(Operation::SetPermissions {
                path: path.clone(),
                mode: new_mode,
            });
        }
        
        if atime.is_some() || mtime.is_some() {
            use std::time::UNIX_EPOCH;
            self.emit(Operation::SetTimestamps {
                path,
                atime: atime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())),
                mtime: mtime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())),
            });
        }
    }
    
    fn on_symlink(&self, parent: u64, name: &OsStr, target: &std::path::Path) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                log::warn!("on_symlink: failed to resolve parent {}", parent);
                return;
            }
        };
        
        self.emit(Operation::SymlinkCreate {
            path,
            target: target.to_path_buf(),
        });
    }
    
    fn on_link(&self, ino: u64, newparent: u64, newname: &OsStr) {
        let existing_path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                log::warn!("on_link: failed to resolve inode {}", ino);
                return;
            }
        };
        
        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                log::warn!("on_link: failed to resolve new parent {}", newparent);
                return;
            }
        };
        
        self.emit(Operation::HardLinkCreate {
            existing_path,
            new_path,
        });
    }
}
```

### OpcodeQueue

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::operations::Opcode;

/// Thread-safe opcode queue with notification.
/// 
/// Uses a `VecDeque` for storage and a `Condvar` for wake-on-push.
/// This design allows inspection of queue contents and future persistence.
pub struct OpcodeQueue {
    /// The actual queue storage
    inner: Mutex<VecDequeInner>,
    /// Condition variable for waking the processor
    not_empty: Condvar,
}

struct VecDequeInner {
    queue: VecDeque<Opcode>,
    capacity: usize,
}

/// Handle for pushing opcodes (can be shared via Arc)
pub struct OpcodeSender {
    queue: Arc<OpcodeQueue>,
}

impl OpcodeQueue {
    /// Create a new queue with default capacity (10,000)
    pub fn new() -> Arc<Self> {
        Self::with_capacity(10_000)
    }
    
    /// Create a new queue with specified capacity
    pub fn with_capacity(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDequeInner {
                queue: VecDeque::with_capacity(capacity.min(1000)), // Pre-alloc reasonable amount
                capacity,
            }),
            not_empty: Condvar::new(),
        })
    }
    
    /// Create a sender handle for this queue
    pub fn sender(self: &Arc<Self>) -> OpcodeSender {
        OpcodeSender {
            queue: Arc::clone(self),
        }
    }
    
    /// Push an opcode onto the queue (non-blocking)
    /// 
    /// Returns `Err(opcode)` if queue is at capacity.
    pub fn try_push(&self, opcode: Opcode) -> Result<(), Opcode> {
        let mut inner = self.inner.lock().unwrap();
        if inner.queue.len() >= inner.capacity {
            return Err(opcode);
        }
        inner.queue.push_back(opcode);
        self.not_empty.notify_one();
        Ok(())
    }
    
    /// Push an opcode, blocking if at capacity
    pub fn push(&self, opcode: Opcode) {
        let mut inner = self.inner.lock().unwrap();
        // If at capacity, we just push anyway (unbounded growth as fallback)
        // A more sophisticated impl could block here
        inner.queue.push_back(opcode);
        self.not_empty.notify_one();
    }
    
    /// Pop an opcode from the queue (non-blocking)
    pub fn try_pop(&self) -> Option<Opcode> {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.pop_front()
    }
    
    /// Pop an opcode, blocking until one is available
    pub fn pop(&self) -> Opcode {
        let mut inner = self.inner.lock().unwrap();
        while inner.queue.is_empty() {
            inner = self.not_empty.wait(inner).unwrap();
        }
        inner.queue.pop_front().unwrap()
    }
    
    /// Drain all available opcodes (non-blocking)
    pub fn drain(&self) -> Vec<Opcode> {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.drain(..).collect()
    }
    
    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().queue.is_empty()
    }
    
    /// Get current queue length
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }
    
    /// Peek at the queue contents (for debugging/inspection)
    pub fn peek_all(&self) -> Vec<Opcode> {
        let inner = self.inner.lock().unwrap();
        inner.queue.iter().cloned().collect()
    }
}

impl OpcodeSender {
    /// Push an opcode onto the queue (non-blocking)
    pub fn try_send(&self, opcode: Opcode) -> Result<(), Opcode> {
        self.queue.try_push(opcode)
    }
    
    /// Push an opcode onto the queue
    pub fn send(&self, opcode: Opcode) {
        self.queue.push(opcode)
    }
}

impl Clone for OpcodeSender {
    fn clone(&self) -> Self {
        Self {
            queue: Arc::clone(&self.queue),
        }
    }
}
```

## Wiring It Together

```rust
use std::sync::Arc;

fn setup_versioned_filesystem(
    source_dir: PathBuf,
    mount_point: PathBuf,
) -> Result<(ObservingFS<PassthroughFS>, Arc<OpcodeQueue>), Error> {
    // Create the passthrough filesystem
    let passthrough = PassthroughFS::new(&source_dir, &mount_point)?;
    
    // Get shared inode map
    let inode_map = passthrough.inode_map();
    
    // Create opcode queue
    let queue = OpcodeQueue::new();
    
    // Create recorder with queue sender
    let recorder = OpcodeRecorder::new(
        inode_map,
        source_dir.clone(),
        queue.sender(),
    );
    
    // Wrap passthrough with observer
    let mut observing = ObservingFS::new(passthrough);
    observing.add_observer(Arc::new(recorder));
    
    Ok((observing, queue))
}

fn main() -> Result<(), Error> {
    let (filesystem, queue) = setup_versioned_filesystem(
        PathBuf::from("/data/working"),
        PathBuf::from("/mnt/ize"),
    )?;
    
    // Spawn processing thread
    std::thread::spawn(move || {
        loop {
            // Blocks until opcode available
            let opcode = queue.pop();
            // Process opcode → Pijul change
            println!("Processing: {:?}", opcode);
        }
    });
    
    // Mount filesystem (blocks)
    let options = vec![MountOption::AutoUnmount];
    fuser::mount2(filesystem, "/mnt/ize", &options)?;
    
    Ok(())
}
```

## Error Handling Strategy

| Error | Handling |
|-------|----------|
| Inode resolution fails | Log warning, skip opcode |
| Queue full (`try_send` fails) | Log warning, opcode dropped (or force push) |
| Metadata lookup fails | Use best guess or skip |
| Channel disconnected | Log error, observer becomes no-op |

## Testing Strategy

### Unit Tests

1. **Path resolution:**
   - Resolve valid inode → correct path
   - Resolve invalid inode → None
   - Resolve with name → parent path + name

2. **Opcode generation:**
   - Each observer method generates correct opcode variant
   - Sequence numbers are monotonic
   - Timestamps are reasonable

3. **Queue behavior:**
   - Opcodes enqueued successfully
   - Queue full behavior (bounded)
   - Multiple senders work

### Integration Tests

1. **End-to-end with mock filesystem:**
   - Create file → FileCreate opcode
   - Write data → FileWrite opcode
   - Delete file → FileDelete opcode
   - Etc.

2. **Stress tests:**
   - Many rapid operations
   - Verify no opcodes lost (within queue capacity)
   - Verify ordering preserved per-path

## Future Enhancements

### 1. Opcode Coalescing

Add a coalescing layer between recorder and queue:

```rust
pub struct CoalescingRecorder {
    inner: OpcodeRecorder,
    pending: RwLock<HashMap<PathBuf, Vec<Opcode>>>,
    flush_interval: Duration,
}
```

Coalesce sequential writes to same file within time window.

### 2. Persistence

Write opcodes to disk before processing:

```rust
pub struct PersistentQueue {
    wal: WriteAheadLog,
    in_memory: OpcodeQueue,
}
```

Enables crash recovery.

### 3. Metrics

Track recorder statistics:

```rust
pub struct RecorderMetrics {
    opcodes_created: AtomicU64,
    opcodes_dropped: AtomicU64,
    resolution_failures: AtomicU64,
    queue_high_watermark: AtomicU64,
}
```

## Summary

The `OpcodeRecorder`:
1. Implements `FsObserver` to receive filesystem notifications
2. Uses shared `InodeMap` to resolve inodes to paths
3. Generates monotonically sequenced `Opcode` instances
4. Non-blocking enqueue via bounded channel
5. Graceful degradation on errors (log and continue)

This design keeps the FUSE thread responsive while ensuring filesystem mutations are captured for versioning.