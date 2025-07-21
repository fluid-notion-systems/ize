# Sanakirja Analysis for Ize

## Overview

Sanakirja is a transactional, copy-on-write B-tree data structure library written in Rust. It's the storage backend for Pijul and provides:

- **ACID transactions** with MVCC (Multi-Version Concurrency Control)
- **Copy-on-write semantics** perfect for versioning
- **Memory-mapped files** for performance
- **Zero-copy deserialization**
- **Crash resistance** with atomic commits

## Architecture Analysis

### Core Components

1. **Environment** - Manages the memory-mapped file and transactions
2. **Transactions** - Read-only or read-write operations
3. **B-trees** - The main data structure for storing key-value pairs
4. **Pages** - Fixed-size blocks in the file (typically 4KB)
5. **Roots** - Named entry points to B-trees

### Key Features for Ize

1. **Built-in Versioning**: Each transaction creates a new version
2. **Efficient Snapshots**: Copy-on-write makes snapshots cheap
3. **Concurrent Readers**: Multiple readers can access old versions
4. **Atomic Operations**: All-or-nothing transaction commits
5. **Space Efficiency**: Shared unchanged pages between versions

## Integration Approaches

### Option A: Direct Sanakirja Integration

**Pros:**
- Immediate access to battle-tested versioning
- No abstraction overhead
- Direct control over transaction boundaries
- Proven in production with Pijul

**Cons:**
- Tight coupling to Sanakirja's API
- Harder to swap backends later
- Must understand Sanakirja internals
- Limited to its data model

**Implementation sketch:**
```rust
use sanakirja::{Env, RootDb, Error};

pub struct SanakirjaStorage {
    env: Arc<Env>,
}

impl SanakirjaStorage {
    pub fn record_operation(&self, path: &str, op: Operation) -> Result<()> {
        let mut txn = self.env.write_txn()?;
        // Store operation in B-tree
        txn.commit()?;
        Ok(())
    }
}
```

### Option B: Trait-Based with Sanakirja Implementation

**Pros:**
- Clean abstraction for testing
- Can swap backends later
- Easier to understand codebase
- Can start simple, optimize later

**Cons:**
- Extra abstraction layer
- Might miss Sanakirja-specific optimizations
- More code to write initially

**Implementation sketch:**
```rust
pub trait VersionedStorage {
    fn record_operation(&mut self, path: &str, op: Operation) -> Result<Version>;
    fn get_file_at_version(&self, path: &str, version: Version) -> Result<FileData>;
    fn list_versions(&self, path: &str) -> Result<Vec<VersionInfo>>;
}

pub struct SanakirjaBackend {
    env: Arc<Env>,
}

impl VersionedStorage for SanakirjaBackend {
    // Implementation using Sanakirja
}
```

## Recommendation: Hybrid Approach

Start with a **minimal trait** but design it with Sanakirja's capabilities in mind:

1. **Phase 1**: Define trait with core operations needed for MVP
2. **Phase 2**: Implement with Sanakirja, learning its patterns
3. **Phase 3**: Optimize trait based on real usage
4. **Phase 4**: Consider direct Sanakirja usage for advanced features

### Proposed Initial Trait

```rust
use std::time::SystemTime;

pub type Version = u64;

#[derive(Debug, Clone)]
pub struct Operation {
    pub op_type: OpType,
    pub path: String,
    pub timestamp: SystemTime,
    pub data: OperationData,
}

#[derive(Debug, Clone)]
pub enum OpType {
    Create,
    Write,
    Delete,
    Rename,
}

#[derive(Debug, Clone)]
pub enum OperationData {
    Content(Vec<u8>),
    Rename { old_path: String, new_path: String },
    None,
}

pub trait VersionedStorage: Send + Sync {
    /// Initialize a new storage backend
    fn new(path: &Path) -> Result<Self> where Self: Sized;

    /// Record a filesystem operation
    fn record_operation(&mut self, op: Operation) -> Result<Version>;

    /// Get file content at a specific version
    fn get_file_at_version(&self, path: &str, version: Version) -> Result<Vec<u8>>;

    /// List all versions for a file
    fn list_versions(&self, path: &str) -> Result<Vec<(Version, Operation)>>;

    /// Get the latest version number
    fn latest_version(&self) -> Version;

    /// Begin a new transaction (for batch operations)
    fn begin_transaction(&mut self) -> Result<Transaction>;
}
```

## Sanakirja-Specific Considerations

### Data Model Design

For Sanakirja, we'll need several B-trees:

1. **Operations Log**: `Version -> Operation`
2. **Path Index**: `Path -> Vec<Version>`
3. **Content Store**: `ContentHash -> Vec<u8>`
4. **Metadata**: `Version -> Metadata`

### Transaction Boundaries

- Each filesystem operation = one transaction
- Batch operations (like recursive copies) in single transaction
- Read operations don't need transactions

### Performance Optimizations

1. **Content Deduplication**: Store file content by hash
2. **Delta Compression**: Store only changes for large files
3. **Lazy Loading**: Don't load full history unless needed
4. **Concurrent Reads**: Multiple threads reading different versions

## Implementation Roadmap

### Week 1: Basic Integration
- [ ] Set up Sanakirja dependency
- [ ] Implement basic trait
- [ ] Simple in-memory backend for testing
- [ ] Basic Sanakirja backend

### Week 2: Core Features
- [ ] Operation recording
- [ ] Version retrieval
- [ ] Path indexing
- [ ] Transaction support

### Week 3: Optimization
- [ ] Content deduplication
- [ ] Performance benchmarks
- [ ] Concurrent access tests
- [ ] Error handling

### Week 4: Advanced Features
- [ ] Garbage collection for old versions
- [ ] Export/import functionality
- [ ] Compression support
- [ ] Migration tools

## Conclusion

**Recommendation**: Start with the trait-based approach but keep it thin and Sanakirja-informed. This gives us:

1. Testability with mock implementations
2. Learning curve for Sanakirja
3. Future flexibility if needed
4. Clean architecture

The trait should be minimal and focused on version control operations, not general storage. We can always bypass the trait for Sanakirja-specific features later.
