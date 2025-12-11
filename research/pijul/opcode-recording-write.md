# Write Opcode Recording: Deep Dive

This document focuses specifically on how a `FileWrite` opcode gets recorded into Pijul's change system.

## The Core Flow

When we receive a `FileWrite { path, offset, data }` opcode, we need to:

1. **Get current file content** from Pijul's pristine (the versioned state)
2. **Apply the write** to create the new content
3. **Diff** old content vs new content
4. **Create a Change** from that diff
5. **Apply the Change** to the pristine

```
FileWrite { path: "foo.txt", offset: 5, data: b"hello" }
                              │
                              ▼
    ┌───────────────────────────────────────────┐
    │ 1. Read "foo.txt" from pristine           │
    │    old_content = output_file(pos) → bytes │
    └─────────────────────┬─────────────────────┘
                          │ old_content = b"Hello world"
                          ▼
    ┌───────────────────────────────────────────┐
    │ 2. Apply write in memory                  │
    │    new_content = old_content.clone()      │
    │    new_content[5..10] = b"hello"          │
    └─────────────────────┬─────────────────────┘
                          │ new_content = b"Hellohello"
                          ▼
    ┌───────────────────────────────────────────┐
    │ 3. Diff old vs new                        │
    │    diff(old_content, new_content)         │
    │    → Replacement { old: 5, len: 5,        │
    │                    new: 5, new_len: 5 }   │
    └─────────────────────┬─────────────────────┘
                          │
                          ▼
    ┌───────────────────────────────────────────┐
    │ 4. Create Change from diff                │
    │    Hunk::Edit { change: NewVertex {...} } │
    └─────────────────────┬─────────────────────┘
                          │
                          ▼
    ┌───────────────────────────────────────────┐
    │ 5. Apply change to pristine               │
    │    apply_local_change(change, hash)       │
    └───────────────────────────────────────────┘
```

---

## Step 1: Reading File Content from Pristine

### The Graph Structure

Pijul stores file content as a **graph of vertices**, not raw bytes. Each vertex represents a chunk of content, and edges connect them.

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
// From libpijul::pristine
pub struct Position<H> {
    pub change: H,        // Which change introduced this
    pub pos: ChangePosition,  // Byte offset within that change
}

pub struct Vertex<H> {
    pub change: H,
    pub start: ChangePosition,
    pub end: ChangePosition,
}
```

### Reading the Content

```rust
use libpijul::output::output_file;
use libpijul::vertex_buffer::Writer;
use libpijul::alive::{retrieve, output_graph};

/// Get file content from pristine
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
        // follow_oldest_path walks the tree to find the file's inode vertex
        t.follow_oldest_path(changes, &c, path)?
    };
    
    // 2. Retrieve the file's content graph
    let mut graph = {
        let t = txn.read();
        let c = channel.read();
        // retrieve() builds a Graph of all alive vertices for this file
        retrieve(&*t, t.graph(&*c), file_pos, false)?
    };
    
    // 3. Output the graph to bytes
    let mut buffer = Vec::new();
    output_file(
        changes,
        txn,
        channel,
        file_pos,
        &mut Writer::new(&mut buffer),
    )?;
    
    Ok(buffer)
}
```

### What `output_file` Does

1. Calls `retrieve()` to build a `Graph` of alive vertices
2. Performs topological sort (handles conflicts, zombies)
3. Outputs each vertex's content in order
4. Handles conflict markers if there are any

---

## Step 2: Applying the Write

This is pure in-memory manipulation:

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

---

## Step 3: Diffing Old vs New

### The Diff Module

Pijul's diff operates on **lines** (or binary chunks), producing `Replacement` records:

```rust
// From libpijul::diff::diff
#[derive(Debug)]
pub struct Replacement {
    pub old: usize,      // Start line in old content
    pub old_len: usize,  // Number of lines deleted
    pub new: usize,      // Start line in new content
    pub new_len: usize,  // Number of lines inserted
}

pub enum Algorithm {
    Myers,          // Classic Myers diff
    Patience,       // Patience diff (better for code)
    ImaraHistogram, // Histogram-based (fast)
}
```

### How Diff Works

```rust
// Simplified from libpijul::diff::mod.rs

impl Recorded {
    pub fn diff<T, C>(
        &mut self,
        changes: &C,
        txn: &ArcTxn<T>,
        channel: &ChannelRef<T>,
        algorithm: Algorithm,
        path: String,
        inode: Inode,
        inode_pos: Position<Option<ChangeId>>,
        old_graph: &mut Graph,      // Graph from pristine
        new_content: &[u8],          // Our new bytes
        encoding: &Option<Encoding>,
        separator: &regex::bytes::Regex,
    ) -> Result<(), DiffError> {
        // 1. Output old graph to get old bytes + position mapping
        let mut diff_buffer = vertex_buffer::Diff::new(inode_pos, path, old_graph);
        output_graph(changes, txn, channel, &mut diff_buffer, old_graph, ...)?;
        
        // 2. Split into lines
        let lines_a = make_old_lines(&diff_buffer, separator);
        let lines_b = make_new_lines(new_content, separator);
        
        // 3. Run diff algorithm
        let replacements = diff::diff(&lines_a, &lines_b, algorithm, false);
        
        // 4. For each replacement, create change atoms
        for r in &replacements {
            if r.old_len > 0 {
                // Delete old content
                self.delete(..., r, ...)?;
            }
            if r.new_len > 0 {
                // Insert new content  
                self.replace(..., r, ...)?;
            }
        }
        
        Ok(())
    }
}
```

---

## Step 4: Creating the Change

### Change Structure

A Change consists of:

```rust
// From libpijul::change

pub struct Change {
    pub hashed: Hashed<Hunk<Option<Hash>, Local>, Author>,
    pub contents: Vec<u8>,   // Raw byte content for new vertices
    // ...
}

pub struct Hashed<H, A> {
    pub header: ChangeHeader,     // Message, author, timestamp
    pub dependencies: Vec<Hash>,   // Changes this depends on
    pub changes: Vec<H>,          // The actual hunks
    pub contents_hash: Hash,      // Hash of contents
    // ...
}

// A Hunk is one logical change
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
    // FileAdd, FileDel, FileMove, etc.
}

// Atom types
pub enum Atom<Change> {
    NewVertex(NewVertex<Change>),  // Insert new content
    EdgeMap(EdgeMap<Change>),      // Delete/modify edges
}

pub struct NewVertex<Change> {
    pub up_context: Vec<Position<Change>>,    // What comes before
    pub down_context: Vec<Position<Change>>,  // What comes after
    pub flag: EdgeFlags,
    pub start: ChangePosition,   // Start in contents buffer
    pub end: ChangePosition,     // End in contents buffer
    pub inode: Position<Change>, // Which file
}
```

### The Recording Process

When diff finds a replacement:

```rust
// From libpijul::diff::replace.rs

impl Recorded {
    pub fn replace(
        &mut self,
        diff: &Diff,
        lines_a: &[Line],
        lines_b: &[Line],
        inode: Inode,
        dd: &D,
        r: usize,
        encoding: &Option<Encoding>,
    ) {
        let old = dd[r].old;
        let old_len = dd[r].old_len;
        let from_new = dd[r].new;
        let new_len = dd[r].new_len;
        
        // 1. Find context (what vertices surround this edit)
        let up_context = get_up_context(diff, lines_a, old);
        let down_context = get_down_context(diff, lines_a, old + old_len);
        
        // 2. Add new bytes to contents buffer
        let start = self.contents.lock().len();
        for line in &lines_b[from_new..(from_new + new_len)] {
            self.contents.lock().extend(line.l);
        }
        let end = self.contents.lock().len();
        self.contents.lock().push(0); // Null terminator
        
        // 3. Create the vertex
        let new_vertex = NewVertex {
            up_context,
            down_context,
            flag: EdgeFlags::BLOCK,
            start: ChangePosition(start.into()),
            end: ChangePosition(end.into()),
            inode: diff.inode,
        };
        
        // 4. Create the hunk
        if old_len > 0 {
            // This is a replacement (delete + insert)
            self.actions.push(Hunk::Replacement {
                change: delete_atom,
                replacement: Atom::NewVertex(new_vertex),
                local: LocalByte { ... },
                encoding: encoding.clone(),
            });
        } else {
            // Pure insertion
            self.actions.push(Hunk::Edit {
                change: Atom::NewVertex(new_vertex),
                local: LocalByte { ... },
                encoding: encoding.clone(),
            });
        }
    }
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

---

## Step 5: Applying the Change

### Saving to ChangeStore

```rust
use libpijul::changestore::filesystem::FileSystem as ChangeStore;

fn save_change(
    changes: &ChangeStore,
    change: &mut Change,
) -> Result<Hash, Error> {
    // Computes hash, compresses, writes to .pijul/changes/XX/XXXXX.change
    changes.save_change(change, |_, _| Ok::<_, Error>(()))
}
```

### Applying to Pristine

```rust
use libpijul::apply::apply_local_change;

fn apply_to_pristine<T>(
    txn: &mut T,
    channel: &ChannelRef<T>,
    change: &Change,
    hash: &Hash,
    updatables: &HashMap<usize, InodeUpdate>,
) -> Result<(u64, Merkle), Error>
where
    T: ChannelMutTxnT + DepsMutTxnT + TreeMutTxnT,
{
    apply_local_change(txn, channel, change, hash, updatables)
}
```

### What `apply_local_change` Does

1. **Registers** the change in the dependency graph
2. **For each NewVertex**: Creates new vertex in the graph, connects edges
3. **For each EdgeMap** (deletion): Marks edges as deleted
4. **Updates** the channel's Merkle tree (for sync)
5. **Updates** inode mappings if files were added/moved

---

## Complete Implementation

```rust
use libpijul::{
    ArcTxn, ChannelRef, Hash, MutTxnT, RecordBuilder,
    Algorithm, DEFAULT_SEPARATOR,
    change::{Change, ChangeHeader},
    changestore::ChangeStore,
    pristine::{ChangeId, Position},
};

impl PijulBackend {
    /// Apply a FileWrite opcode
    pub fn apply_file_write(
        &self,
        path: &str,
        offset: u64,
        data: &[u8],
        timestamp_ns: u64,
    ) -> Result<Option<Hash>, PijulError> {
        // 1. Begin transaction
        let txn = self.pristine.arc_txn_begin()?;
        let channel = self.load_channel(&txn)?;
        
        // 2. Get current content from pristine
        let mut content = self.get_file_content(&txn, &channel, path)?;
        
        // 3. Apply the write
        let offset = offset as usize;
        let end = offset + data.len();
        if end > content.len() {
            content.resize(end, 0);
        }
        content[offset..end].copy_from_slice(data);
        
        // 4. Record the change using Memory working copy
        let memory = libpijul::working_copy::memory::Memory::new();
        
        // Add all existing files to memory (for context)
        self.populate_memory(&txn, &channel, &memory, Some(path))?;
        
        // Add the modified file with new content
        memory.add_file(path, content);
        
        // 5. Build and record the change
        let mut builder = RecordBuilder::new();
        builder.record(
            txn.clone(),
            Algorithm::Myers,
            false,
            &DEFAULT_SEPARATOR,
            channel.clone(),
            &memory,
            &self.changes,
            path,  // Only diff this file
            1,     // Single-threaded
        )?;
        
        let recorded = builder.finish();
        
        // 6. Check if anything changed
        if recorded.actions.is_empty() {
            return Ok(None);
        }
        
        // 7. Create the change
        let header = ChangeHeader {
            message: format!("write to {} at offset {}", path, offset),
            authors: vec![],
            description: None,
            timestamp: jiff::Timestamp::from_nanosecond(timestamp_ns as i128)
                .unwrap_or_else(|_| jiff::Timestamp::now()),
        };
        
        let contents = std::mem::take(&mut *recorded.contents.lock());
        let globalized = {
            let t = txn.read();
            recorded.actions
                .into_iter()
                .map(|h| h.globalize(&*t).unwrap())
                .collect()
        };
        
        let mut change = Change::make_change(
            &*txn.read(),
            &channel,
            globalized,
            contents,
            header,
            Vec::new(),
        )?;
        
        // 8. Save to changestore
        let hash = self.changes.save_change(&mut change, |_, _| Ok::<_, std::io::Error>(()))?;
        
        // 9. Apply to pristine
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
        
        // 10. Commit transaction
        txn.commit()?;
        
        Ok(Some(hash))
    }
    
    fn get_file_content<T: ChannelTxnT + TreeTxnT>(
        &self,
        txn: &ArcTxn<T>,
        channel: &ChannelRef<T>,
        path: &str,
    ) -> Result<Vec<u8>, PijulError> {
        let t = txn.read();
        let c = channel.read();
        
        // Find file position
        let (pos, _) = t.follow_oldest_path(&self.changes, &c, path)
            .map_err(|e| PijulError::FileNotFound(path.to_string()))?;
        
        // Output content
        let mut buffer = Vec::new();
        libpijul::output::output_file(
            &self.changes,
            txn,
            channel,
            pos,
            &mut libpijul::vertex_buffer::Writer::new(&mut buffer),
        ).map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
        
        Ok(buffer)
    }
    
    fn populate_memory<T: ChannelTxnT + TreeTxnT>(
        &self,
        txn: &ArcTxn<T>,
        channel: &ChannelRef<T>,
        memory: &libpijul::working_copy::memory::Memory,
        exclude: Option<&str>,
    ) -> Result<(), PijulError> {
        // Memory working copy needs to have the directory structure
        // for the file we're modifying
        if let Some(path) = exclude {
            // Create parent directories
            let mut current = String::new();
            for component in path.split('/') {
                if !current.is_empty() {
                    current.push('/');
                }
                current.push_str(component);
                if current != path {
                    memory.add_dir(&current);
                }
            }
        }
        Ok(())
    }
}
```

---

## Key Insights

### 1. Content is Never Stored Directly

Pijul doesn't store file content directly. It stores:
- **Vertices**: Chunks of content introduced by changes
- **Edges**: How vertices connect (and which are deleted)

### 2. Context is Everything

New content is positioned by **context** (up/down), not byte offset. This is why patches can be applied out of order.

### 3. The `contents` Buffer

New bytes are appended to a `contents: Vec<u8>` buffer. `NewVertex.start` and `NewVertex.end` are offsets into this buffer.

### 4. Changes are Self-Contained

A Change includes:
- All hunks (what to do)
- All new content bytes
- Dependencies (which changes must be applied first)

### 5. Recording vs Applying

- **Recording**: Creates a Change from a diff (what we do)
- **Applying**: Takes a Change and modifies the pristine

---

## Error Cases

| Scenario | What Happens |
|----------|--------------|
| File doesn't exist | `follow_oldest_path` fails with `PathNotFound` |
| File deleted | `retrieve` returns empty graph, diff creates full content |
| Write extends file | `content.resize()` handles it |
| Empty write | `recorded.actions` is empty, return `Ok(None)` |
| Concurrent writes | Transaction isolation prevents conflicts |

---

## Testing

```rust
#[test]
fn test_write_middle_of_file() {
    let backend = setup_backend();
    
    // Create initial file
    backend.apply_file_create("test.txt", 0o644, b"Hello world").unwrap();
    
    // Write in the middle
    let hash = backend.apply_file_write("test.txt", 6, b"Rust", 0).unwrap();
    assert!(hash.is_some());
    
    // Verify content
    let content = backend.get_file_content_for_test("test.txt").unwrap();
    assert_eq!(content, b"Hello Rustd");  // "world" partially overwritten
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
    
    // Write same content
    let hash = backend.apply_file_write("test.txt", 0, b"Hello", 0).unwrap();
    assert!(hash.is_none());  // No change recorded
}
```

---

## References

- `libpijul/src/change.rs` - Change structure and creation
- `libpijul/src/diff/mod.rs` - Diff algorithm entry point
- `libpijul/src/diff/replace.rs` - How replacements become hunks
- `libpijul/src/record.rs` - RecordBuilder and Recorded types
- `libpijul/src/output/mod.rs` - Reading from pristine
- `libpijul/src/apply.rs` - Applying changes
- `libpijul/src/working_copy/memory.rs` - In-memory working copy