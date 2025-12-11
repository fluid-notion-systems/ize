# Opcode Recording: Deep Dive into libpijul Integration

This document provides a comprehensive analysis of how Ize's opcode system integrates with libpijul to record filesystem changes into the version control system.

## Table of Contents

1. [Overview](#overview)
2. [Pijul's Data Model](#pijuls-data-model)
3. [The Recording Flow](#the-recording-flow)
4. [Operation Implementations](#operation-implementations)
5. [PijulBackend Implementation](#pijulbackend-implementation)
6. [Error Handling](#error-handling)
7. [Testing Strategy](#testing-strategy)

---

## Overview

### The Problem

When a filesystem operation occurs (via FUSE), we need to:
1. **Immediately** write to `working/` (handled by PassthroughFS)
2. **Asynchronously** record the change in `.pijul/pristine` (handled by OpcodeProcessor)

The challenge: by the time we process opcode N, the working directory may already reflect operations N+1, N+2, etc. We cannot read from `working/` to determine the "before" state.

### The Solution

Use libpijul's `Memory` working copy to construct a virtual filesystem state representing exactly the change we want to record:

```
┌─────────────────────────────────────────────────────────────┐
│                    Opcode Processing Flow                    │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Opcode { op: FileWrite { path, offset, data } }            │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 1. Read current content from pristine (output_file)   │  │
│  └───────────────────────────────────────────────────────┘  │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 2. Apply the operation in memory                       │  │
│  │    content[offset..offset+len] = data                  │  │
│  └───────────────────────────────────────────────────────┘  │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 3. Create Memory working copy with new content         │  │
│  │    memory.add_file(path, new_content)                  │  │
│  └───────────────────────────────────────────────────────┘  │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 4. Record change (diff pristine vs Memory)             │  │
│  │    builder.record(..., &memory, ...)                   │  │
│  └───────────────────────────────────────────────────────┘  │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 5. Save change to changestore                          │  │
│  │    changes.save_change(&mut change, ...)               │  │
│  └───────────────────────────────────────────────────────┘  │
│                         │                                   │
│                         ▼                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ 6. Apply change to pristine                            │  │
│  │    apply_local_change(txn, channel, change, hash, ..)  │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## Pijul's Data Model

### Content is Stored as a Graph

Pijul doesn't store file content directly as bytes. Instead, it stores:
- **Vertices**: Chunks of content introduced by changes
- **Edges**: How vertices connect (and which are deleted)

```
File "foo.txt" in pristine:

   ROOT
     │
     ▼
  ┌─────────────────┐
  │ Vertex A        │
  │ "Hello "        │
  │ change: abc123  │
  │ pos: 0..6       │
  └────────┬────────┘
           │
           ▼
  ┌─────────────────┐
  │ Vertex B        │
  │ "world"         │
  │ change: def456  │
  │ pos: 0..5       │
  └─────────────────┘
```

### Key Types

```rust
// Position in a change
pub struct Position<H> {
    pub change: H,            // Which change introduced this
    pub pos: ChangePosition,  // Byte offset within that change
}

// A vertex (chunk of content)
pub struct Vertex<H> {
    pub change: H,
    pub start: ChangePosition,
    pub end: ChangePosition,
}

// New content being added
pub struct NewVertex<Change> {
    pub up_context: Vec<Position<Change>>,    // What comes before
    pub down_context: Vec<Position<Change>>,  // What comes after
    pub flag: EdgeFlags,
    pub start: ChangePosition,   // Start in contents buffer
    pub end: ChangePosition,     // End in contents buffer
    pub inode: Position<Change>, // Which file
}
```

### Context: The Key Concept

**Context** is how Pijul knows where to insert new content:
- `up_context`: Position(s) that the new content comes **after**
- `down_context`: Position(s) that the new content comes **before**

```
Before:
  [Vertex A: "Hello "] ──────► [Vertex B: "world"]
  
After inserting "beautiful " between them:
  [Vertex A: "Hello "] ──► [NEW: "beautiful "] ──► [Vertex B: "world"]
  
The NEW vertex has:
  up_context = [end of Vertex A]
  down_context = [start of Vertex B]
```

This context-based approach is what makes Pijul's patches **commutative** - patches to different parts of a file can be applied in any order.

### Change Structure

A Change consists of Hunks:

```rust
pub struct Change {
    pub hashed: Hashed<Hunk<Option<Hash>, Local>, Author>,
    pub contents: Vec<u8>,   // Raw byte content for new vertices
}

pub enum Hunk<Atom, Local> {
    Edit {
        change: Atom,           // The vertex change
        local: Local,           // Line number info for display
        encoding: Option<Encoding>,
    },
    Replacement {
        change: Atom,           // Delete atom
        replacement: Atom,      // Insert atom
        local: Local,
        encoding: Option<Encoding>,
    },
    FileAdd { ... },
    FileDel { ... },
    FileMove { ... },
    // etc.
}

pub enum Atom<Change> {
    NewVertex(NewVertex<Change>),  // Insert new content
    EdgeMap(EdgeMap<Change>),      // Delete/modify edges
}
```

---

## The Recording Flow

### Step 1: Read Current Content from Pristine

```rust
fn get_file_content<T, C>(
    changes: &C,
    txn: &ArcTxn<T>,
    channel: &ChannelRef<T>,
    path: &str,
) -> Result<Vec<u8>, Error>
where
    T: ChannelTxnT + TreeTxnT,
    C: ChangeStore,
{
    // 1. Find the file's position in the graph
    let (file_pos, _ambiguous) = {
        let t = txn.read();
        let c = channel.read();
        t.follow_oldest_path(changes, &c, path)?
    };
    
    // 2. Output the file content to a buffer
    let mut buffer = Vec::new();
    libpijul::output::output_file(
        changes,
        txn,
        channel,
        file_pos,
        &mut libpijul::vertex_buffer::Writer::new(&mut buffer),
    )?;
    
    Ok(buffer)
}
```

### Step 2: Apply Operation in Memory

For a write operation:

```rust
fn apply_write(content: &mut Vec<u8>, offset: u64, data: &[u8]) {
    let offset = offset as usize;
    let end = offset + data.len();
    
    // Extend if write goes beyond current length
    if end > content.len() {
        content.resize(end, 0);
    }
    
    // Apply the write
    content[offset..end].copy_from_slice(data);
}
```

### Step 3: Create Memory Working Copy

```rust
let memory = libpijul::working_copy::memory::Memory::new();

// Create parent directories
if let Some(parent) = path.parent() {
    memory.create_dir_all(&parent_path)?;
}

// Add the modified file
memory.add_file(path, new_content);
```

### Step 4: Record the Change

```rust
let mut builder = libpijul::RecordBuilder::new();
builder.record(
    txn.clone(),
    libpijul::Algorithm::Myers,  // Diff algorithm
    false,                        // stop_early
    &libpijul::DEFAULT_SEPARATOR, // Line separator
    channel.clone(),
    &memory,                      // Our virtual working copy
    &changes,
    path,                         // Scope to just this file
    1,                            // Single-threaded
)?;

let recorded = builder.finish();
```

### Step 5: Save to Changestore

```rust
// Globalize actions (convert local IDs to hashes)
let globalized: Vec<_> = {
    let t = txn.read();
    recorded.actions
        .into_iter()
        .map(|h| h.globalize(&*t).unwrap())
        .collect()
};

let contents = recorded.contents.lock().clone();

let mut change = Change::make_change(
    &*txn.read(),
    &channel,
    globalized,
    contents,
    ChangeHeader {
        message: format!("write to {}", path),
        authors: vec![],
        description: None,
        timestamp: jiff::Timestamp::now(),
    },
    Vec::new(),
)?;

let hash = changes.save_change(&mut change, |_, _| Ok::<_, Error>(()))?;
```

### Step 6: Apply to Pristine

```rust
{
    let mut t = txn.write();
    libpijul::apply::apply_local_change(
        &mut *t,
        &channel,
        &change,
        &hash,
        &recorded.updatables,
    )?;
}

txn.commit()?;
```

---

## Operation Implementations

### FileWrite

The most common operation. See the complete flow above.

**Key points:**
- Read current content from pristine
- Apply write to cloned content
- Diff produces `Hunk::Edit` or `Hunk::Replacement`

### FileCreate

```rust
fn apply_file_create(
    &self,
    path: &str,
    mode: u32,
    content: &[u8],
) -> Result<Option<Hash>, Error> {
    let txn = self.pristine.arc_txn_begin()?;
    let channel = self.load_channel(&txn)?;
    
    // Register the file in the tree
    {
        let mut t = txn.write();
        t.add_file(path, self.salt)?;
    }
    
    // Create Memory with the new file
    let memory = Memory::new();
    memory.add_file(path, content.to_vec());
    
    // Record and apply
    // ... (same as write flow)
}
```

**Produces:** `Hunk::FileAdd` with `add_name`, `add_inode`, and `contents` atoms.

### FileTruncate

```rust
fn apply_file_truncate(
    &self,
    path: &str,
    new_size: u64,
) -> Result<Option<Hash>, Error> {
    // Get current content
    let mut content = self.get_file_content(&txn, &channel, path)?;
    
    // Truncate
    let new_size = new_size as usize;
    if new_size < content.len() {
        content.truncate(new_size);
    } else if new_size > content.len() {
        content.resize(new_size, 0);
    } else {
        return Ok(None); // No change
    }
    
    // Record with truncated content
    // ...
}
```

### FileDelete

```rust
fn apply_file_delete(
    &self,
    path: &str,
) -> Result<Option<Hash>, Error> {
    // Create Memory WITHOUT the file
    let memory = Memory::new();
    // (populate with other files if needed, but not this one)
    
    // Remove from tree
    {
        let mut t = txn.write();
        t.remove_file(path)?;
    }
    
    // Record produces Hunk::FileDel
    // ...
}
```

**Produces:** `Hunk::FileDel` with deletion atom.

### FileRename

```rust
fn apply_file_rename(
    &self,
    old_path: &str,
    new_path: &str,
) -> Result<Option<Hash>, Error> {
    // Get content from old location
    let content = self.get_file_content(&txn, &channel, old_path)?;
    
    // Move in tree
    {
        let mut t = txn.write();
        t.move_file(old_path, new_path, self.salt)?;
    }
    
    // Create Memory with file at new location
    let memory = Memory::new();
    memory.add_file(new_path, content);
    
    // Record with empty prefix to capture full tree change
    // ...
}
```

**Produces:** `Hunk::FileMove` with `del` and `add` atoms.

### DirCreate

```rust
fn apply_dir_create(
    &self,
    path: &str,
    mode: u32,
) -> Result<Option<Hash>, Error> {
    // Register in tree
    {
        let mut t = txn.write();
        t.add_dir(path, self.salt)?;
    }
    
    // Note: Empty directories may not produce a change
    // Pijul only tracks directories that contain files
}
```

### Metadata Operations

**SetPermissions:**
- Pijul only tracks the executable bit (`mode & 0o100`)
- Other permission changes are no-ops for version control

**SetTimestamps:**
- Pijul doesn't track timestamps
- No-op, just log

**SetOwnership:**
- Pijul doesn't track ownership
- No-op, just log

### Symbolic Links

Pijul doesn't have native symlink support. Workaround options:
1. Store as marker file with content `SYMLINK:{target}`
2. Ignore with warning

### Hard Links

Hard links are incompatible with content-addressed VCS. The new path is recorded as an independent file with the same content.

---

## PijulBackend Implementation

### Struct Definition

```rust
pub struct PijulBackend {
    /// Path to .pijul directory
    pijul_dir: PathBuf,
    /// Path to working directory
    working_dir: PathBuf,
    /// The pristine database
    pristine: Pristine,
    /// Change store for persisting changes
    changes: ChangeStore,
    /// Current channel name
    current_channel: String,
    /// Salt for inode generation
    salt: u64,
}
```

### Main Apply Method

```rust
impl PijulBackend {
    pub fn apply_opcode(&self, opcode: &Opcode) -> Result<Option<Hash>, PijulError> {
        let timestamp = opcode.timestamp();
        
        match opcode.op() {
            Operation::FileCreate { path, mode, content } => {
                self.apply_file_create(path, *mode, content, timestamp)
            }
            Operation::FileWrite { path, offset, data } => {
                self.apply_file_write(path, *offset, data, timestamp)
            }
            Operation::FileTruncate { path, new_size } => {
                self.apply_file_truncate(path, *new_size, timestamp)
            }
            Operation::FileDelete { path } => {
                self.apply_file_delete(path, timestamp)
            }
            Operation::FileRename { old_path, new_path } => {
                self.apply_file_rename(old_path, new_path, timestamp)
            }
            Operation::DirCreate { path, mode } => {
                self.apply_dir_create(path, *mode, timestamp)
            }
            Operation::DirDelete { path } => {
                self.apply_dir_delete(path, timestamp)
            }
            Operation::DirRename { old_path, new_path } => {
                self.apply_dir_rename(old_path, new_path, timestamp)
            }
            Operation::SetPermissions { path, mode } => {
                self.apply_set_permissions(path, *mode, timestamp)
            }
            Operation::SetTimestamps { .. } => Ok(None), // No-op
            Operation::SetOwnership { .. } => Ok(None),  // No-op
            Operation::SymlinkCreate { path, target } => {
                self.apply_symlink_create(path, target, timestamp)
            }
            Operation::SymlinkDelete { path } => {
                self.apply_file_delete(path, timestamp)
            }
            Operation::HardLinkCreate { existing_path, new_path } => {
                self.apply_hardlink_create(existing_path, new_path, timestamp)
            }
        }
    }
}
```

### Path Conversion

libpijul uses `/`-separated paths without leading slashes:

```rust
fn path_to_pijul(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches("./")
        .replace(std::path::MAIN_SEPARATOR, "/")
}
```

---

## Error Handling

### Error Types

```rust
#[derive(Error, Debug)]
pub enum PijulError {
    #[error("Sanakirja database error: {0}")]
    Sanakirja(#[from] SanakirjaError),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),
    
    #[error("Transaction error: {0}")]
    Transaction(String),
    
    #[error("Change store error: {0}")]
    ChangeStore(String),
    
    #[error("File not found in pristine: {0}")]
    FileNotFound(String),
}
```

### Error Recovery

| Error Type | Strategy |
|------------|----------|
| `FileNotFound` | Skip opcode (file might have been deleted) |
| `Transaction` | Retry with backoff, then fail |
| `ChangeStore` | Critical - alert and pause processing |
| `Io` | Retry, then log and continue |

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_write_middle_of_file() {
    let backend = setup_backend();
    
    backend.apply_file_create("test.txt", 0o644, b"Hello world").unwrap();
    
    let hash = backend.apply_file_write("test.txt", 6, b"Rust", 0).unwrap();
    assert!(hash.is_some());
    
    let content = backend.get_file_content_for_test("test.txt").unwrap();
    assert_eq!(content, b"Hello Rustd");
}

#[test]
fn test_write_extends_file() {
    let backend = setup_backend();
    
    backend.apply_file_create("test.txt", 0o644, b"Hi").unwrap();
    backend.apply_file_write("test.txt", 2, b" there!", 0).unwrap();
    
    let content = backend.get_file_content_for_test("test.txt").unwrap();
    assert_eq!(content, b"Hi there!");
}

#[test]
fn test_write_no_change() {
    let backend = setup_backend();
    
    backend.apply_file_create("test.txt", 0o644, b"Hello").unwrap();
    
    let hash = backend.apply_file_write("test.txt", 0, b"Hello", 0).unwrap();
    assert!(hash.is_none()); // No change recorded
}
```

---

## Implementation Checklist

### Phase 1: Core File Operations
- [ ] `FileCreate` - new file with content
- [ ] `FileWrite` - write at offset
- [ ] `FileTruncate` - resize file
- [ ] `FileDelete` - remove file
- [ ] `FileRename` - move/rename file

### Phase 2: Directory Operations
- [ ] `DirCreate` - create directory
- [ ] `DirDelete` - remove directory
- [ ] `DirRename` - move/rename directory

### Phase 3: Metadata
- [ ] `SetPermissions` - chmod (exec bit only)
- [ ] `SetTimestamps` - no-op logging
- [ ] `SetOwnership` - no-op logging

### Phase 4: Links
- [ ] `SymlinkCreate` - marker file workaround
- [ ] `SymlinkDelete` - same as file delete
- [ ] `HardLinkCreate` - copy file content

### Phase 5: Testing
- [ ] Unit tests for each operation
- [ ] Integration tests
- [ ] Edge case tests

---

## References

- `libpijul/src/change.rs` - Change structure and creation
- `libpijul/src/diff/mod.rs` - Diff algorithm entry point
- `libpijul/src/diff/replace.rs` - How replacements become hunks
- `libpijul/src/record.rs` - RecordBuilder and Recorded types
- `libpijul/src/output/mod.rs` - Reading from pristine
- `libpijul/src/apply.rs` - Applying changes
- `libpijul/src/working_copy/memory.rs` - In-memory working copy