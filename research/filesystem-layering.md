# Filesystem Layering Architecture for Ize

## Problem Statement

We want to compose multiple FUSE filesystem behaviors:

1. **PassthroughFS**: The "real" filesystem that passes operations through to `working/`
2. **OpcodeRecordingFS**: Records filesystem operations (opcodes) for async persistence to Pijul
3. **FanOutFS**: Potentially fans out to multiple "slave" filesystems

The challenge: **Read operations** need to return data from exactly one source, while **write operations** might need to notify multiple components.

## Analysis of FUSE Reply Semantics

The `fuser` crate's reply types (`ReplyData`, `ReplyWrite`, etc.) are **consuming** - once you call `reply.data(...)` or `reply.written(...)`, the reply is consumed. This means:

- You can't pass the same reply to multiple filesystem implementations
- Only ONE component can actually respond to the kernel

This fundamentally shapes our architecture.

## Architecture Options

### Option 1: FanOutFS with Primary/Secondary (Problematic)

```rust
struct FanOutFS {
    primary: Box<dyn Filesystem>,
    secondaries: Vec<Box<dyn Filesystem>>,
}
```

**Problems:**
- Can't pass the same Reply to multiple handlers
- Would need to create "dummy" replies for secondaries
- Secondaries would need to be modified to not actually reply
- Complex, error-prone

**Verdict:** ❌ Not recommended

---

### Option 2: Decorator/Wrapper Pattern (Simple Case)

```rust
struct OpcodeRecordingFS<F: Filesystem> {
    inner: F,
    opcode_queue: Arc<OpCodeQueue>,
}

impl<F: Filesystem> Filesystem for OpcodeRecordingFS<F> {
    fn write(&mut self, req: &Request, ino: u64, fh: u64, 
             offset: i64, data: &[u8], ..., reply: ReplyWrite) {
        // 1. Record opcode FIRST (with data copy)
        self.record_write(ino, offset, data);
        
        // 2. Delegate to inner
        self.inner.write(req, ino, fh, offset, data, ..., reply)
    }
    
    fn read(&mut self, ..., reply: ReplyData) {
        // Just delegate - reads don't need recording
        self.inner.read(..., reply)
    }
}
```

**Pros:**
- Simple, clean
- Single responsibility
- No Reply juggling
- Works with existing PassthroughFS unchanged

**Cons:**
- Limited to single-level wrapping
- Recording happens synchronously (before delegation)

**Verdict:** ✅ Good for the simple case (what we need now)

---

### Option 3: Observer Pattern (Recommended for Flexibility)

Separate the "observation" concern from the "filesystem" concern entirely:

```rust
/// Observer trait - receives notifications, doesn't handle replies
pub trait FsObserver: Send + Sync {
    fn on_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]);
    fn on_create(&self, parent: u64, name: &OsStr, mode: u32);
    fn on_unlink(&self, parent: u64, name: &OsStr);
    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32);
    fn on_rmdir(&self, parent: u64, name: &OsStr);
    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr);
    fn on_setattr(&self, ino: u64, mode: Option<u32>, size: Option<u64>, ...);
    // etc.
}

/// A filesystem that notifies observers of mutations
pub struct ObservingFS<F: Filesystem> {
    inner: F,
    observers: Vec<Arc<dyn FsObserver>>,
}

impl<F: Filesystem> Filesystem for ObservingFS<F> {
    fn write(&mut self, req: &Request, ino: u64, fh: u64,
             offset: i64, data: &[u8], write_flags: u32,
             flags: i32, lock_owner: Option<u64>, reply: ReplyWrite) {
        // Notify all observers (non-blocking, just copies data)
        for observer in &self.observers {
            observer.on_write(ino, fh, offset, data);
        }
        
        // Delegate to inner filesystem (handles the actual reply)
        self.inner.write(req, ino, fh, offset, data, write_flags, flags, lock_owner, reply)
    }
    
    fn read(&mut self, req: &Request, ino: u64, fh: u64,
            offset: i64, size: u32, flags: i32,
            lock_owner: Option<u64>, reply: ReplyData) {
        // Reads just delegate - no observation needed
        self.inner.read(req, ino, fh, offset, size, flags, lock_owner, reply)
    }
    
    // ... other operations follow the same pattern
}
```

**Then OpcodeRecordingObserver implements FsObserver:**

```rust
pub struct OpcodeRecordingObserver {
    opcode_queue: Arc<OpCodeQueue>,
    /// Needs path resolution - could be shared with PassthroughFS
    path_resolver: Arc<dyn PathResolver>,
}

impl FsObserver for OpcodeRecordingObserver {
    fn on_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]) {
        // Resolve inode to path
        let path = match self.path_resolver.resolve(ino) {
            Some(p) => p,
            None => {
                warn!("on_write: could not resolve inode {}", ino);
                return;
            }
        };
        
        // Build opcode
        let opcode = OpCode {
            id: None,
            op_type: OpType::FileWrite {
                offset: offset as u64,
                size: data.len() as u64,
            },
            timestamp: current_timestamp(),
            path,
            target_path: None,
            data: OpData::Content(data.to_vec()),
            metadata: FileMetadata::default(),
            parent_id: None,
        };
        
        // Enqueue (non-blocking)
        if let Err(e) = self.opcode_queue.try_enqueue(opcode) {
            error!("Failed to enqueue write opcode: {}", e);
        }
    }
    
    // ... other on_* methods
}
```

**Pros:**
- Clean separation of concerns
- Multiple observers supported naturally
- Observers are simple - just receive data, no reply handling
- Easy to test observers in isolation
- Path resolution can be shared

**Cons:**
- Slightly more indirection
- Need to define PathResolver trait or share state

**Verdict:** ✅✅ Recommended - most flexible and clean

---

### Option 4: Event-Based with Channels

```rust
pub enum FsEvent {
    Write { ino: u64, fh: u64, offset: i64, data: Vec<u8> },
    Create { parent: u64, name: OsString, mode: u32, ino: u64 },
    Unlink { parent: u64, name: OsString },
    // ...
}

pub struct EventEmittingFS<F: Filesystem> {
    inner: F,
    event_tx: mpsc::Sender<FsEvent>,
}
```

**Pros:**
- Fully async
- Decoupled via channel
- Easy to add multiple consumers

**Cons:**
- More complex
- Channel overhead
- Need to handle channel full/disconnected

**Verdict:** 🤔 Good for async processing, but Observer pattern is simpler for our use case

---

## Recommended Architecture

For Ize, I recommend **Option 3: Observer Pattern** with the following structure:

```
┌─────────────────────────────────────────────────────────┐
│                      FUSE Kernel                         │
└─────────────────────────┬───────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────┐
│                    ObservingFS<F>                        │
│  ┌─────────────────────────────────────────────────┐    │
│  │  observers: Vec<Arc<dyn FsObserver>>            │    │
│  │    ├── OpcodeRecordingObserver ──► OpCodeQueue  │    │
│  │    └── (future: MetricsObserver, etc.)          │    │
│  └─────────────────────────────────────────────────┘    │
│                          │                               │
│                          ▼                               │
│  ┌─────────────────────────────────────────────────┐    │
│  │              inner: PassthroughFS               │    │
│  │                      │                          │    │
│  │                      ▼                          │    │
│  │               working/ directory                │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

## Path Resolution Challenge

One challenge: `OpcodeRecordingObserver` needs to resolve inodes to paths, but `PassthroughFS` maintains that mapping internally.

### Solution: Shared PathResolver

```rust
/// Trait for resolving inodes to paths
pub trait PathResolver: Send + Sync {
    fn resolve(&self, ino: u64) -> Option<PathBuf>;
    fn resolve_with_name(&self, parent_ino: u64, name: &OsStr) -> Option<PathBuf>;
}

/// PassthroughFS exposes its inode map through this trait
impl PathResolver for PassthroughFS {
    fn resolve(&self, ino: u64) -> Option<PathBuf> {
        self.get_path_for_inode(ino)
    }
    
    fn resolve_with_name(&self, parent_ino: u64, name: &OsStr) -> Option<PathBuf> {
        self.get_path_for_inode(parent_ino)
            .map(|p| p.join(name))
    }
}
```

But there's a lifetime issue - `ObservingFS` owns `PassthroughFS`, and observers need a reference to it.

### Solution: Arc<RwLock<...>> for Shared State

```rust
/// Shared inode-to-path mapping
pub type InodeMap = Arc<RwLock<HashMap<u64, PathBuf>>>;

pub struct PassthroughFS {
    // ... other fields
    inode_to_path: InodeMap,  // Now an Arc, can be shared
}

impl PassthroughFS {
    pub fn inode_map(&self) -> InodeMap {
        Arc::clone(&self.inode_to_path)
    }
}

pub struct OpcodeRecordingObserver {
    opcode_queue: Arc<OpCodeQueue>,
    inode_map: InodeMap,  // Shared with PassthroughFS
}
```

## Operations Classification

### Mutation Operations (Need Observer Notification)

| FUSE Method | Observer Method | Data Captured |
|-------------|-----------------|---------------|
| `write` | `on_write` | ino, offset, data bytes |
| `create` | `on_create` | parent, name, mode, (resulting ino) |
| `mkdir` | `on_mkdir` | parent, name, mode |
| `unlink` | `on_unlink` | parent, name |
| `rmdir` | `on_rmdir` | parent, name |
| `rename` | `on_rename` | parent, name, newparent, newname |
| `setattr` | `on_setattr` | ino, changed attributes |
| `symlink` | `on_symlink` | parent, name, target |
| `link` | `on_link` | ino, newparent, newname |

### Read-Only Operations (No Notification)

| FUSE Method | Reason |
|-------------|--------|
| `lookup` | Read-only query |
| `getattr` | Read-only query |
| `read` | Read-only (no state change) |
| `readdir` | Read-only query |
| `readlink` | Read-only query |
| `open` | Just opens, doesn't modify |
| `opendir` | Just opens, doesn't modify |
| `release` | Cleanup, no data change |
| `releasedir` | Cleanup, no data change |
| `flush` | Just sync, no new data |
| `fsync` | Just sync, no new data |
| `access` | Permission check |
| `statfs` | Stats query |

## Implementation Plan

### Phase 1: Extract PathResolver
1. Make `inode_to_path` in `PassthroughFS` an `Arc<RwLock<...>>`
2. Add `inode_map()` getter method
3. Ensure all tests still pass

### Phase 2: Implement FsObserver Trait
```rust
// In src/filesystems/observer.rs
pub trait FsObserver: Send + Sync {
    fn on_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]) {}
    fn on_create(&self, parent: u64, name: &OsStr, mode: u32, result_ino: Option<u64>) {}
    fn on_unlink(&self, parent: u64, name: &OsStr) {}
    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32, result_ino: Option<u64>) {}
    fn on_rmdir(&self, parent: u64, name: &OsStr) {}
    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {}
    fn on_setattr(&self, ino: u64, size: Option<u64>, mode: Option<u32>, 
                  atime: Option<SystemTime>, mtime: Option<SystemTime>) {}
}
```

### Phase 3: Implement ObservingFS Wrapper
```rust
// In src/filesystems/observing.rs
pub struct ObservingFS<F: Filesystem> {
    inner: F,
    observers: Vec<Arc<dyn FsObserver>>,
}

impl<F: Filesystem> ObservingFS<F> {
    pub fn new(inner: F) -> Self {
        Self { inner, observers: Vec::new() }
    }
    
    pub fn add_observer(&mut self, observer: Arc<dyn FsObserver>) {
        self.observers.push(observer);
    }
}
```

### Phase 4: Implement OpcodeRecordingObserver
```rust
// In src/filesystems/opcode_observer.rs
pub struct OpcodeRecordingObserver {
    queue: Arc<OpCodeQueue>,
    inode_map: InodeMap,
}

impl FsObserver for OpcodeRecordingObserver {
    // ... implement all on_* methods
}
```

### Phase 5: Wire It Together
```rust
// In mount setup code
let passthrough = PassthroughFS::new(working_dir, mount_point)?;
let inode_map = passthrough.inode_map();

let opcode_observer = Arc::new(OpcodeRecordingObserver::new(
    opcode_queue.clone(),
    inode_map,
));

let mut observing_fs = ObservingFS::new(passthrough);
observing_fs.add_observer(opcode_observer);

fuser::mount2(observing_fs, mount_point, &options)?;
```

## Alternative: Notification After Success

One consideration: should observers be notified **before** or **after** the operation succeeds?

**Before (current design):**
- Pro: Observer has data even if operation fails (for logging)
- Con: May record ops that actually failed

**After:**
- Pro: Only records successful operations
- Con: Harder to implement (need to capture reply success)

For Ize's use case (versioning), we probably want **after** - only record successful writes. But this is tricky because the `reply` is consumed.

### Solution: Success Callback

```rust
impl<F: Filesystem> Filesystem for ObservingFS<F> {
    fn write(&mut self, req: &Request, ino: u64, fh: u64,
             offset: i64, data: &[u8], ..., reply: ReplyWrite) {
        // Capture data for observers
        let captured_data = data.to_vec();
        let observers = self.observers.clone();
        
        // Wrap reply to intercept success
        let notifying_reply = NotifyingReplyWrite::new(reply, move |written| {
            // Only called on success
            for observer in &observers {
                observer.on_write(ino, fh, offset, &captured_data[..written as usize]);
            }
        });
        
        self.inner.write(req, ino, fh, offset, data, ..., notifying_reply)
    }
}
```

This requires creating wrapper reply types, which adds complexity. For now, **notify before** is simpler and acceptable - failed writes are rare and can be handled at the queue processing stage.

## Conclusion

The **Observer Pattern** (Option 3) provides the cleanest architecture for Ize's filesystem layering needs:

1. **Clear separation**: PassthroughFS handles I/O, observers handle side effects
2. **No Reply juggling**: Only the inner filesystem deals with replies
3. **Extensible**: Easy to add more observers (metrics, audit logging, etc.)
4. **Testable**: Observers can be tested independently
5. **Matches the mental model**: "Observe filesystem changes and record them"

This is fundamentally different from a "FanOutFS" because we're not fanning out filesystem operations - we're observing them and recording metadata. The actual filesystem operation only happens once, in PassthroughFS.