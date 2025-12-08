# Pijul Backend Migration Analysis

## Executive Summary

This document analyzes the migration of Ize's storage backend to Pijul's Sanakirja engine, and addresses the critical architectural questions around directory presentation, shadowing, and command passthrough.

## Current Architecture

### How Ize Works Today

```
Source Directory: /home/user/project/
├── file1.txt
├── dir/
│   └── file2.txt
└── Ize.db              # SQLite database (currently a stub)

Mount Point: /mnt/ize-project/
├── file1.txt           # Passthrough to source
└── dir/
    └── file2.txt       # Passthrough to source
```

Key characteristics:
- **Separate mount point**: Source and mount are different directories
- **Passthrough model**: FUSE reads/writes directly to source files
- **Database inside source**: `Ize.db` lives in the source directory
- **Database hidden**: FUSE filters `Ize.db` from directory listings

### Current Database Hiding Implementation

The current code in `passthrough.rs` hides the database file during `readdir`:

```rust
// From readdir() implementation
for entry in entries {
    let file_name = entry.file_name();
    
    // Skip the database file if we're in the root directory
    if ino == ROOT_INODE
        && file_name == self.db_path.file_name().unwrap_or_default()
    {
        debug!("readdir: skipping database file {:?}", file_name);
        continue;
    }
    // ... process entry
}
```

This approach:
- Only hides the single `Ize.db` file
- Only works for root directory (won't hide `.ize/` subdirectory)
- Doesn't prevent direct access via `lookup()` if path is known

### Current Code Flow

```rust
// From passthrough.rs
impl PassthroughFS {
    pub fn new(db_path: P, mount_point: Q) -> Result<Self> {
        // Source dir = parent of db_path
        // Mount point = separate location
        // Validation: db cannot be inside mount point
    }
    
    pub fn real_path(&self, path: &Path) -> PathBuf {
        // Converts virtual path to real path in source directory
        self.path_manager.get_real_path(path)
    }
}
```

---

## Pijul/Sanakirja Storage Architecture

### Sanakirja Overview

Sanakirja is a transactional, copy-on-write B+ tree database with:

| Feature | Benefit for Ize |
|---------|-----------------|
| Memory-mapped files | Efficient large file handling |
| Copy-on-write | Cheap snapshots/branching |
| ACID transactions | Crash-safe filesystem operations |
| MVCC | Concurrent readers with writers |
| Fork operation | O(1) repository branching |

### Pijul's Directory Structure

```
.pijul/
├── pristine/           # Sanakirja database files
│   └── db              # B+ tree storage
├── changes/            # Individual patch files (content-addressed)
│   ├── ABC123...       # Patch files
│   └── DEF456...
├── config              # Repository configuration
└── remotes/            # Remote repository tracking
```

### Using Pijul's Structure Directly: Considerations

**Potential Benefits:**
- Leverage existing, battle-tested code
- Compatibility with Pijul tooling
- Well-designed for version control semantics

**Problems with Direct Use:**
1. **Patch-centric model**: Pijul stores *changes* as first-class citizens, not file snapshots. Ize needs file-level versioning.
2. **Working copy assumption**: Pijul assumes files are on disk, not intercepted via FUSE
3. **No real-time tracking**: Pijul requires explicit `record` commands
4. **Different granularity**: Pijul tracks line-level changes; Ize tracks byte-level writes

**Recommended Approach**: Use Sanakirja directly (the storage layer) but design Ize-specific schema rather than adopting Pijul's patch graph model.

### Key Insight: Pijul's Model

Pijul doesn't use FUSE. Instead:
1. Files exist as regular files in the working directory
2. `.pijul/pristine/` stores the *graph representation* of file history
3. Operations like `pijul record` compare working directory to pristine state
4. Changes are computed as patches, stored content-addressed

**For Ize, we want FUSE + Pijul's storage model.**

---

## Directory Presentation Options

### Option A: Separate Mount Point (Current Model)

```
Source:  /home/user/project/
         ├── files...
         └── .ize/
             └── pristine/

Mount:   /mnt/ize/project/     # DIFFERENT location
         └── files...          # .ize hidden
```

**Pros:**
- Simple implementation (current)
- No shadowing issues
- Database access is straightforward

**Cons:**
- Two locations for same project
- User must remember to use mount point
- IDE/editor might access source directly, bypassing versioning

### Option B: Overlay Mount (Same Location)

```
Before mount:
/home/user/project/
├── files...
└── .ize/

After mount (FUSE at same path):
/home/user/project/        # FUSE mount
├── files...               # Through FUSE
└── .ize/                  # HIDDEN or special handling
```

**Pros:**
- Single location - intuitive for users
- All file access goes through FUSE
- Works with existing workflows/IDEs

**Cons:**
- **Shadowing problem**: Original directory is hidden by mount
- **Recursive access risk**: FUSE accessing its own storage through mount
- Complex implementation

### Option C: External Database Location

```
Source:  /home/user/project/
         └── files only (no .ize)

Storage: /var/lib/ize/projects/{hash}/
         └── pristine/

Mount:   /home/user/project/  # Or separate location
```

**Pros:**
- Clean separation of data and metadata
- No shadowing of database
- Easier backup strategies

**Cons:**
- Association between project and storage is external
- Moving projects breaks the link
- More complex project discovery

### Option D: Hybrid with Bind Mount (Recommended)

```
Source:  /home/user/project/
         ├── files...
         └── .ize/ → symlink or bind mount to external storage

Storage: ~/.local/share/ize/{project-id}/
         └── pristine/

Mount:   /home/user/project-ize/   # Versioned view
         └── files...              # .ize hidden
```

**Pros:**
- Database is external (no shadowing)
- Source directory stays clean
- Mount point is explicit

**Cons:**
- Symlink/bind mount complexity
- Multiple locations to manage

---

## The Shadowing Problem: Deep Analysis

### What is Shadowing?

When FUSE mounts at `/path/dir`, the original contents of `/path/dir` become inaccessible through that path. The FUSE filesystem "shadows" the original directory.

```
Pre-mount state:
/project/
├── file.txt     # Real file at /project/file.txt

Post-mount (FUSE at /project/):
/project/
├── file.txt     # Now served by FUSE, not the real file

# The real file still exists but is inaccessible via /project/
```

### Why This Matters for Ize

If we mount FUSE at the source directory location:

1. **FUSE needs to access source files** - to read/write actual content
2. **Source files are shadowed** - they're behind the FUSE mount
3. **Deadlock potential** - FUSE → kernel → FUSE (infinite loop)

### Solutions to Shadowing

#### Solution 1: File Descriptor Preservation

```rust
use std::os::unix::io::{AsRawFd, OwnedFd, FromRawFd};
use libc::{openat, O_PATH, O_DIRECTORY, O_RDONLY, O_WRONLY};

impl PassthroughFS {
    fn new(source_dir: &Path) -> Result<Self> {
        // Open source directory BEFORE mounting
        // O_PATH gives us a reference without opening for read/write
        let source_fd = unsafe {
            let fd = libc::open(
                source_dir.as_ptr(),
                O_PATH | O_DIRECTORY
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };
        
        Self {
            source_dir_fd: source_fd,
            // ...
        }
    }
    
    fn open_real_file(&self, relative_path: &Path, flags: i32) -> Result<File> {
        // Use openat() with the preserved FD
        // This accesses the REAL file, bypassing the FUSE mount
        let path_cstr = CString::new(relative_path.as_os_str().as_bytes())?;
        let fd = unsafe {
            openat(
                self.source_dir_fd.as_raw_fd(),
                path_cstr.as_ptr(),
                flags
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
```

**Critical Detail**: The `openat()` syscall with a directory FD resolves paths *relative to that FD's directory*, completely bypassing the VFS layer (and thus FUSE) for path resolution.

**How it works:**
- Open the source directory before mounting
- Store the file descriptor
- Use `openat()` family of calls with that FD
- These calls access the *real* files, not through FUSE

**Pros:**
- Elegant solution
- No bind mounts needed
- Kernel handles it correctly

**Cons:**
- Must be done before mount
- FD must be kept alive
- Some edge cases with path resolution

#### Solution 2: Bind Mount Pre-mount

```bash
# Before FUSE mount
mount --bind /home/user/project /run/ize/shadow/project

# Now FUSE can mount at original location
ize mount /home/user/project

# FUSE accesses files via /run/ize/shadow/project
```

**Pros:**
- Clear separation
- Standard Linux technique
- Works reliably

**Cons:**
- Requires mount privileges
- More complex setup
- Cleanup on unmount

#### Solution 3: Database Outside Source

```
Source:  /home/user/project/       # Pure working files
Storage: ~/.ize/projects/{id}/     # Database here
Mount:   /home/user/project/       # FUSE here
```

FUSE accesses source files through preserved FD (Solution 1) and database through the external path.

**This is the recommended approach for Ize.**

---

## Command Passthrough Analysis

### The Question

When a user runs commands inside the mounted directory:
```bash
cd /mnt/ize/project
git status
cargo build
```

Should Ize:
1. Just track the file operations? (Current approach)
2. Intercept and understand the commands?
3. Provide special handling for certain tools?

### Analysis

#### Level 1: Transparent Tracking (Recommended Starting Point)

All commands work normally. Ize sees:
- File reads (git reading .git/, cargo reading Cargo.toml)
- File writes (cargo creating target/, git updating index)

**Pros:**
- Simple, reliable
- Works with any tool
- No special cases

**Cons:**
- Lots of noise (temp files, build artifacts)
- No semantic understanding

#### Level 2: Filtered Tracking

Add patterns to ignore:
```toml
# .ize/config
[ignore]
patterns = [
    "target/",
    "node_modules/",
    ".git/",
    "*.tmp",
]
```

**Pros:**
- Reduces noise
- Familiar from .gitignore
- User-configurable

**Cons:**
- Must maintain patterns
- Might miss important changes

#### Level 3: Semantic Command Detection (Future)

Detect when known commands run and record semantically:
```
Operation: "cargo build" 
Time: 2024-01-15T10:30:00
Files modified: [target/debug/...] (bulk, can be regenerated)
```

**Pros:**
- Rich history understanding
- Smart restoration ("rebuild after restore")

**Cons:**
- Complex implementation
- Heuristic-based detection
- Edge cases

### Recommendation

Start with **Level 1 + Level 2**: Transparent tracking with ignore patterns. Defer semantic command detection to later phases.

---

## Proposed Ize Architecture with Sanakirja

### Directory Layout

```
Project Directory:
/home/user/project/
├── src/
├── Cargo.toml
└── .ize/
    └── config          # Just config, points to storage

Storage Directory:
~/.local/share/ize/projects/{project-uuid}/
├── pristine/           # Sanakirja database
│   └── db              # B+ tree files
├── content/            # Content-addressed file storage
│   ├── ab/
│   │   └── cdef123...  # SHA-256 addressed content
│   └── ...
└── snapshots/          # Named snapshot metadata

Mount Point:
/home/user/project-ize/     # Or configured location
├── src/                    # Versioned view
├── Cargo.toml
└── .ize/                   # HIDDEN from listing
```

### Sanakirja Schema Design

```rust
use sanakirja::btree;

/// Main database structure
pub struct IzeDB {
    env: sanakirja::Env,
}

/// Database tables (B+ trees)
impl IzeDB {
    // Root page slots
    const ROOT_FILES: usize = 0;      // PathId -> FileRecord
    const ROOT_DIRS: usize = 1;       // PathId -> DirRecord  
    const ROOT_CONTENT: usize = 2;    // ContentHash -> offset
    const ROOT_OPERATIONS: usize = 3; // OpId -> Operation
    const ROOT_PATH_INDEX: usize = 4; // String -> PathId
    const ROOT_HISTORY: usize = 5;    // PathId -> Vec<OpId>
}

/// File record in database
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FileRecord {
    pub path_id: u64,
    pub content_hash: [u8; 32],  // SHA-256
    pub size: u64,
    pub mode: u32,
    pub mtime: u64,
    pub last_op_id: u64,
}

/// Operation record
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpRecord {
    pub op_id: u64,
    pub op_type: u8,        // Create, Write, Delete, etc.
    pub path_id: u64,
    pub timestamp: u64,
    pub prev_content_hash: [u8; 32],
    pub new_content_hash: [u8; 32],
    pub metadata: u64,      // Offset to extended metadata
}
```

### Core Operations

```rust
impl IzeDB {
    /// Record a file operation
    pub fn record_operation(&mut self, op: &Operation) -> Result<OpId> {
        let mut txn = self.env.mut_txn_begin()?;
        
        // 1. Store content if new
        let content_hash = self.store_content_if_new(&mut txn, &op.content)?;
        
        // 2. Create operation record
        let op_id = self.next_op_id(&mut txn)?;
        let op_record = OpRecord {
            op_id,
            op_type: op.op_type as u8,
            path_id: self.get_path_id(&mut txn, &op.path)?,
            timestamp: op.timestamp,
            prev_content_hash: self.get_current_hash(&txn, &op.path)?,
            new_content_hash: content_hash,
            metadata: 0,
        };
        
        // 3. Insert into operations table
        let mut ops_db = txn.root_db(Self::ROOT_OPERATIONS)?;
        btree::put(&mut txn, &mut ops_db, &op_id, &op_record)?;
        
        // 4. Update file record
        self.update_file_record(&mut txn, &op.path, &content_hash, op_id)?;
        
        // 5. Update history index
        self.append_to_history(&mut txn, op_record.path_id, op_id)?;
        
        txn.commit()?;
        Ok(op_id)
    }
    
    /// Create a snapshot (cheap due to COW)
    pub fn create_snapshot(&mut self, name: &str) -> Result<SnapshotId> {
        let mut txn = self.env.mut_txn_begin()?;
        
        // Fork all tables - O(1) due to copy-on-write
        let files_fork = txn.fork(&self.get_db(Self::ROOT_FILES)?)?;
        let dirs_fork = txn.fork(&self.get_db(Self::ROOT_DIRS)?)?;
        // ... fork other tables
        
        // Store snapshot metadata
        let snapshot_id = self.store_snapshot_metadata(&mut txn, name, /* refs */)?;
        
        txn.commit()?;
        Ok(snapshot_id)
    }
    
    /// Get file at specific operation
    pub fn get_file_at_op(&self, path: &str, op_id: OpId) -> Result<FileContent> {
        let txn = self.env.txn_begin()?;
        
        // Find the operation
        let ops_db = txn.root_db(Self::ROOT_OPERATIONS)?;
        let op_record: OpRecord = btree::get(&txn, &ops_db, &op_id, None)?
            .ok_or(Error::OpNotFound)?;
        
        // Get content by hash
        let content = self.get_content(&txn, &op_record.new_content_hash)?;
        
        Ok(content)
    }
}
```

### FUSE Integration

```rust
pub struct IzeFuseFS {
    // Source directory access (FD-based to avoid shadowing)
    source_dir_fd: OwnedFd,
    
    // Database connection
    db: IzeDB,
    
    // Path manager (inode mapping)
    path_manager: PathManager,
    
    // Configuration
    config: IzeConfig,
}

impl IzeFuseFS {
    pub fn new(source_dir: &Path, storage_path: &Path) -> Result<Self> {
        // Open source directory FD BEFORE mounting
        let source_dir_fd = open(source_dir, O_PATH | O_DIRECTORY)?;
        
        // Open/create database
        let db = IzeDB::open_or_create(storage_path)?;
        
        Ok(Self {
            source_dir_fd,
            db,
            path_manager: PathManager::new(),
            config: IzeConfig::load(source_dir)?,
        })
    }
    
    /// Access real file using FD (bypasses FUSE)
    fn open_real_file(&self, path: &Path, flags: i32) -> Result<File> {
        let fd = openat(
            self.source_dir_fd.as_raw_fd(),
            path,
            flags
        )?;
        Ok(File::from(fd))
    }
}

impl Filesystem for IzeFuseFS {
    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, 
             offset: i64, data: &[u8], /* ... */) -> Result<u32> {
        let path = self.path_manager.get_path(ino)?;
        
        // Write to real file (via FD, bypasses FUSE)
        let mut file = self.open_real_file(&path, O_WRONLY)?;
        file.seek(SeekFrom::Start(offset as u64))?;
        let written = file.write(data)?;
        
        // Record operation asynchronously
        self.db.record_operation(&Operation {
            op_type: OpType::Write,
            path: path.to_string_lossy().into(),
            timestamp: now(),
            content: None, // Will be computed from file
            offset: Some(offset),
            size: Some(data.len()),
        })?;
        
        Ok(written as u32)
    }
}
```

---

## Migration Plan

### Phase 1: Sanakirja Integration (Week 1-2)

1. Add Sanakirja dependency
2. Implement basic schema (files, operations)
3. Create `IzeDB` wrapper with simple operations
4. Unit tests for database operations

### Phase 2: Storage Trait Adaptation (Week 2-3)

1. Update `Storage` trait to match Sanakirja capabilities
2. Implement `SanakirjaStorage` backend
3. Add content-addressed storage for file data
4. Integration tests

### Phase 3: FUSE Refactoring (Week 3-4)

1. Implement FD-based source access
2. Add async operation recording
3. Update `PassthroughFS` to use new storage
4. Handle shadowing for same-location mounts

### Phase 4: Advanced Features (Week 5-6)

1. Implement snapshot/fork operations
2. Add history queries
3. Implement file restoration
4. Performance optimization

### Phase 5: Polish (Week 7-8)

1. Ignore patterns implementation
2. Configuration system
3. CLI improvements
4. Documentation

---

## Open Questions

### 1. Mount Point Location

**Question**: Should Ize support mounting at the source location (overlay style)?

**Recommendation**: Start with separate mount point, add overlay support later.

### 2. Content Storage

**Question**: Store full file content or deltas?

**Recommendation**: Start with full content (content-addressed). Add delta compression later for large files.

### 3. Real-time vs Batch

**Question**: Record operations immediately or batch?

**Recommendation**: Async queue with immediate fsync option for critical files.

### 4. Large File Handling

**Question**: How to handle files larger than RAM?

**Recommendation**: Stream to content-addressed storage, use mmap for retrieval.

---

## Conclusion

Migrating to Sanakirja provides:
1. **Efficient snapshots** via copy-on-write forking
2. **ACID transactions** for crash safety
3. **Proven technology** from Pijul VCS
4. **Natural fit** for version control semantics

The shadowing problem is solvable via **file descriptor preservation** combined with **external database storage**. This allows mounting at or near the source directory while maintaining clean access to both source files and version database.

Recommended architecture:
- Database in `~/.local/share/ize/projects/{id}/`
- Source directory contains only `.ize/config` 
- Mount point separate from source (initially)
- FD-based source access for overlay support (future)

---

## Appendix: Key Differences from Pijul

| Aspect | Pijul | Ize |
|--------|-------|-----|
| **Interface** | CLI commands | FUSE filesystem |
| **Tracking** | Explicit `record` | Automatic on write |
| **Granularity** | Line-level patches | Byte-level operations |
| **Primary Unit** | Patch (change) | Operation (event) |
| **File Model** | Graph of lines | Content-addressed blobs |
| **Merge** | Category theory pushouts | N/A (linear history) |
| **Working Copy** | Regular files | FUSE-intercepted files |

**What we take from Pijul:**
- Sanakirja storage engine
- Content-addressing for deduplication  
- Copy-on-write for cheap snapshots
- Transaction model

**What we don't take:**
- Patch theory / commutative changes
- Line-level graph model
- Explicit recording workflow
- Merge/conflict resolution