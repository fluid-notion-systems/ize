# Pijul Storage Architecture Analysis: A Deep Dive into Elegant Madness

## Overview

Pijul represents a fascinating paradox in the world of version control: it's simultaneously the most mathematically rigorous and the most practically usable VCS ever created. Like a quantum physicist who also happens to be an excellent chef, Pijul combines abstract category theory with brutally pragmatic engineering decisions. It's as if someone took Git, fed it pure mathematics for breakfast, and taught it to actually make sense.

## The Beautiful Madness of Patch Theory

### What Pijul Actually Solves

While Git treats commits as snapshots (like taking polaroid photos of your code), Pijul treats changes as **patches** - atomic units of intention. This is like the difference between:

- **Git**: "Here's what my kitchen looked like at 3pm"
- **Pijul**: "I added salt to the soup, then my roommate added pepper, and somehow we never disagreed about seasoning"

The mathematical foundation is **category theory pushouts**, which sounds terrifying but basically means: "What's the simplest way two people can work on the same thing without stepping on each other's toes?" It's conflict resolution for introverts.

### The Graph Structure: Files as Conversations

```
Traditional filesystems think linearly:
A → B → C → D

Pijul thinks conversationally:
    A ──→ D
   ╱ ╲   ╱
  B   ╲ ╱
   ╲   C
    ╲ ╱
     E

Where each arrow is labeled with "according to patch X, this comes before that"
```

This is profoundly absurd and brilliant: **Pijul stores not just what your file contains, but the entire argument about how it got that way.**

## Sanakirja: The Storage Engine That Doesn't Hate You

### Core Architecture

Sanakirja is Pijul's storage backend, and it's basically what you'd get if you asked a functional programmer to design a database:

```rust
// The beautiful simplicity that hides incredible complexity
let forked_db = transaction.fork(&mut rng, &original_db)?;
```

That single line creates an entire **parallel universe** of your data. It's like `git branch` but if `git branch` actually worked the way you think it should.

### Memory-Mapped Madness

```
Traditional databases: "Let me load this 10GB file into RAM real quick"
Sanakirja: "What if... we just pretended the file WAS memory?"
Operating System: "You magnificent bastard, that's exactly what mmap is for"
```

Sanakirja uses 4KB pages (the same size as memory pages) because **it wants to be friends with your CPU cache**. It's like architectural feng shui for data structures.

### Copy-on-Write: The Philosophy of Lazy Updates

```rust
// This is not a copy, this is a promise to copy later if needed
let mut new_page = old_page.clone_if_modified();
```

Copy-on-write in Sanakirja is like promising to clean your room "when someone comes over." Except the OS actually follows through, and your room (data) stays clean forever.

## How Pijul Stores File Operations: A Love Letter to Determinism

### The Patch Graph Storage

Every file in Pijul is stored as a **directed graph** where:
- **Vertices** = lines of text (or chunks of binary data)
- **Edges** = "according to patch X, this chunk comes before that chunk"
- **Labels** = patch identifiers and edge states (alive/dead)

```
File: "Hello World"

Stored as:
Vertex₁: "Hello"
Vertex₂: " "  
Vertex₃: "World"

Edge₁: (Vertex₁ → Vertex₂, patch=abc123, state=alive)
Edge₂: (Vertex₂ → Vertex₃, patch=abc123, state=alive)
```

When you delete "World" and add "Universe":
- Edge₂ becomes `state=dead` 
- New Vertex₄: "Universe"
- New Edge₃: (Vertex₂ → Vertex₄, patch=def456, state=alive)

**The old data never disappears. It just becomes historically accurate instead of currently relevant.**

### Content Addressing: The Hash Table of Truth

```rust
// Every chunk of content gets a cryptographic address
let content_id = ContentHash::from_data(&file_content);
storage.store_content(content_id, file_content)?;

// Later retrieval is just a lookup
let content = storage.get_content(content_id)?;
```

This is like giving every paragraph in every book ever written a unique postal address. Except the postal system is run by mathematicians and never loses mail.

### The Repository Structure: Organized Chaos

```
.pijul/
├── pristine/          # The actual database files
│   └── db/           # Sanakirja's B+ tree files
├── changes/          # Individual patch files
│   ├── ABC123...     # Patch files (content-addressed)
│   └── DEF456...
└── working_copy/     # Your actual files (reconstructed)
    └── state         # Mapping from pristine to working copy
```

The working copy is essentially a **cached view** of the database. When you run `pijul record`, it figures out what patches to create by diffing the working copy against the pristine state.

## The Implementation Details That Make You Weep (Good Tears)

### Reference Counting for Fork Operations

```rust
// When you fork a database
pub fn fork<R: Rng>(&mut self, rng: &mut R, db: &Db<K, V>) -> Result<Db<K, V>> {
    // This just increments reference counts
    // No data is actually copied until you modify something
    self.rc_db.fork(rng, &db.0)
}
```

The reference counting system is stored **in another B+ tree** to make it transactional. It's reference counting all the way down, except when it's B+ trees all the way down.

### The Allocator: Memory Management as Art

Sanakirja includes its own page allocator that:
- Manages 4KB blocks in the memory-mapped file
- Maintains a free list (also stored as a B+ tree, naturally)
- Handles transactions by not updating references until commit
- Supports rollback by simply not updating the root pointer

```rust
// Simplified view of allocation
pub struct Allocator {
    free_list: Db<u64, ()>,    // Free page numbers
    allocated: Db<u64, ()>,    // Allocated page numbers  
    root_page: u64,            // Where everything starts
}
```

It's like having a librarian who keeps perfect track of every book, never loses anything, and can instantly tell you if a book is available - except the library is your entire filesystem.

### Transaction Isolation: The Time Travel Problem

```rust
// Multiple readers can see different versions simultaneously
let txn1 = env.txn_begin()?;  // Sees state at time T1
let txn2 = env.txn_begin()?;  // Sees state at time T2 (potentially different)

// Write transaction modifies fork of current state
let mut write_txn = env.mut_txn_begin()?;
write_txn.put(&mut table, "key", "value")?;
write_txn.commit()?;  // Atomically updates root pointer
```

This is **snapshot isolation** without the overhead of copying data. Each transaction gets its own view of reality, and reality only updates when someone commits.

## Why This Matters for Claris-FUSE

### Perfect Fit for Filesystem Versioning

1. **Append-Only Operations**: Filesystem changes are naturally append-only (like Pijul patches)
2. **Branching Semantics**: Filesystem snapshots are just forks
3. **Content Addressing**: Deduplication of identical file content
4. **Efficient Diffs**: Natural representation of file changes over time

### Storage Schema for Filesystem Operations

```rust
// Proposed Claris-FUSE storage using Sanakirja primitives
pub struct ClarisStorage {
    // Core file system state
    files: Db<PathId, FileRecord>,
    directories: Db<PathId, DirectoryRecord>,
    content: Db<ContentHash, Vec<u8>>,
    
    // Version control layer  
    file_patches: Db<PathId, PatchSet>,
    patch_graph: Db<PatchId, PatchNode>,
    
    // Indexes for fast queries
    path_to_id: Db<String, PathId>,
    content_refs: Db<ContentHash, Vec<PathId>>,
    timestamp_index: Db<Timestamp, Vec<PatchId>>,
}

#[derive(Clone)]
pub struct FileRecord {
    content_hash: ContentHash,
    metadata: FileMetadata,
    patch_id: PatchId,  // Last patch that modified this file
}

#[derive(Clone)]  
pub struct PatchNode {
    patch_type: PatchType,
    dependencies: Vec<PatchId>,
    timestamp: u64,
    content_delta: Option<ContentDelta>,
}
```

### The Fork Operation for Filesystem Snapshots

```rust
// Creating a filesystem snapshot is just forking the storage
impl ClarisStorage {
    pub fn create_snapshot(&self) -> Result<ClarisStorage> {
        let mut txn = self.env.mut_txn_begin()?;
        
        Ok(ClarisStorage {
            files: txn.fork(&mut self.rng, &self.files)?,
            directories: txn.fork(&mut self.rng, &self.directories)?,
            content: txn.fork(&mut self.rng, &self.content)?,
            // ... fork all tables
        })
    }
    
    pub fn apply_opcode(&mut self, opcode: OpCode) -> Result<PatchId> {
        let mut txn = self.env.mut_txn_begin()?;
        
        // Convert filesystem operation to patch
        let patch = self.opcode_to_patch(opcode)?;
        
        // Store patch in graph structure
        let patch_id = self.next_patch_id();
        txn.put(&mut self.patch_graph, patch_id, patch)?;
        
        // Update file/directory state
        self.apply_patch_to_state(&mut txn, patch_id, &patch)?;
        
        txn.commit()?;
        Ok(patch_id)
    }
}
```

## The Absurdist Beauty of It All

Here's what's wonderful and absurd about using Pijul's storage for a filesystem:

1. **Every file operation becomes a mathematical proof** of what happened
2. **Undo is just applying the inverse patch** (which always exists)
3. **Merge conflicts are impossible** because the math won't let them happen
4. **Time travel is a database query** (`SELECT * FROM patches WHERE timestamp < T`)
5. **Deduplication is automatic** because identical content has identical hashes

### The Implementation Philosophy

Pijul's approach to storage is like asking: "What if we built a database where every operation was reversible, every state was reproducible, and every conflict was mathematically resolvable?"

The answer is Sanakirja: a storage engine that treats data like patches, patches like mathematical objects, and mathematical objects like first-class citizens in a functional programming language.

## Extracting Sanakirja for Claris-FUSE

### The Liberation Strategy

1. **Extract Core Components**:
   ```bash
   # From Pijul repository
   cp -r libpijul/src/pristine/sanakirja ./claris-storage/
   ```

2. **Simplify the API**:
   ```rust
   // Hide the complexity behind filesystem-specific operations
   pub struct FileSystemDB {
       sanakirja_env: sanakirja::Env,
   }
   
   impl FileSystemDB {
       pub fn store_file_operation(&mut self, op: FileOperation) -> Result<()>;
       pub fn get_file_at_time(&self, path: &Path, time: Timestamp) -> Result<FileContent>;
       pub fn create_snapshot(&self) -> Result<FileSystemDB>;
   }
   ```

3. **Add Filesystem-Specific Optimizations**:
   - Path normalization and compression
   - Content deduplication
   - Efficient directory traversal
   - Metadata indexing

### The Beautiful Madness Continues

Using Pijul's storage for a filesystem is like using a Formula 1 race car as your daily driver: 
- Completely overkill for most operations
- Absolutely perfect for the operations that matter
- Makes you feel like you're living in the future
- Occasionally makes you question your life choices, but in a good way

The result will be a filesystem where every operation is reversible, every state is reproducible, and every question about "what happened to my file" has a mathematically precise answer.

It's version control for your entire file system, built on category theory, powered by functional programming, and somehow both the most complex and the most elegant solution possible.

**In other words: it's perfect.**