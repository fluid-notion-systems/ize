# Rust Persistence Engines Analysis for Claris-FUSE

## Overview

This document analyzes Rust persistence engines for implementing a high-performance, embedded version control storage layer. We evaluate engines based on performance, reliability, API design, and suitability for filesystem operations.

## Requirements for Claris-FUSE

### Core Requirements
- **Embedded**: No external dependencies, single-file deployment
- **ACID Transactions**: Consistent filesystem state across operations
- **High Performance**: Sub-millisecond writes, efficient range queries
- **Concurrent Access**: Multiple readers, single writer model
- **Crash Safety**: Survive unexpected shutdowns without corruption
- **Cross-Platform**: Linux, macOS, Windows support

### Filesystem-Specific Needs
- **Efficient Range Queries**: For directory listings and file history
- **Binary Data Support**: Raw file content storage
- **Snapshot Consistency**: Point-in-time filesystem views
- **Incremental Updates**: Delta storage for file changes
- **Memory Efficiency**: Handle large files without excessive RAM usage

## Persistence Engine Comparison

### 1. Sanakirja (Pijul's Engine)

**Architecture**: Custom B+ tree implementation with copy-on-write semantics

```rust
// Sanakirja usage pattern
use sanakirja::{Env, Commit};

let env = Env::new(&path, 1 << 30)?; // 1GB max size
let mut txn = env.mut_txn_begin()?;
let mut table = txn.create_db()?;

// Insert operation
txn.put(&mut table, b"key", b"value")?;
txn.commit()?;

// Fork for snapshots
let fork_env = env.fork()?;
```

**Pros**:
- **Battle-tested**: Powers Pijul version control system
- **Copy-on-Write**: Natural fit for filesystem versioning
- **Fork Operation**: Efficient repository branching/snapshots
- **Transactional**: Full ACID compliance with rollback
- **Custom B+ Trees**: Optimized for patch-based operations
- **Memory-mapped**: Efficient large dataset handling

**Cons**:
- **Complex API**: Steep learning curve, low-level operations
- **Limited Documentation**: Fewer examples and tutorials
- **Pijul-specific**: Optimized for VCS use cases
- **Manual Memory Management**: Requires careful lifetime handling

**Performance Profile**:
- Read: ~1-5μs for key lookup
- Write: ~10-50μs for transaction commit
- Range queries: Excellent due to B+ tree structure
- Memory usage: Low overhead, mmap-based

**Verdict**: **Excellent candidate** - Purpose-built for version control, proven in production

### 2. ReDB

**Architecture**: Modern B+ tree with write-ahead logging and MVCC

```rust
// ReDB usage pattern
use redb::{Database, ReadableTable, TableDefinition};

const MY_TABLE: TableDefinition<&str, &str> = TableDefinition::new("my_table");

let db = Database::create(path)?;
let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_table(MY_TABLE)?;
    table.insert("key", "value")?;
}
write_txn.commit()?;
```

**Pros**:
- **Excellent Performance**: Fastest in most benchmarks
- **Simple API**: Easy to use, high-level abstractions
- **Type Safety**: Compile-time schema validation
- **Pure Rust**: No C dependencies, memory safe
- **Active Development**: Rapidly improving with new features
- **Good Documentation**: Clear examples and API docs

**Cons**:
- **Young Project**: API still evolving, format may change
- **Limited Advanced Features**: No built-in compression or custom comparators
- **Single Writer**: No concurrent write transactions
- **Large Files**: Less optimized for very large values

**Performance Profile**:
- Read: ~500ns for cached lookup
- Write: ~2-10μs for transaction commit
- Range queries: Very fast due to modern B+ tree
- Memory usage: Moderate, with configurable cache

**Verdict**: **Strong candidate** - Best performance, but consider API stability

### 3. Sled

**Architecture**: Log-structured merge tree with eventual consistency

```rust
// Sled usage pattern
use sled::{Db, Tree};

let db = sled::open(path)?;
let tree = db.open_tree("filesystem_ops")?;

// Atomic operations
tree.insert(b"path", b"content")?;
tree.flush()?; // Ensure durability

// Transactions
db.transaction(|tx_db| {
    let tree = tx_db.open_tree("ops")?;
    tree.insert(b"key1", b"val1")?;
    tree.insert(b"key2", b"val2")?;
    Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
})?;
```

**Pros**:
- **Mature**: Years of production use, stable API
- **Feature-rich**: Transactions, merge operators, subscriptions
- **Good Performance**: Optimized for write-heavy workloads
- **Reactive**: Built-in change notifications
- **Robust**: Extensive testing and fuzzing
- **Multiple Trees**: Namespace isolation

**Cons**:
- **Maintenance Concerns**: Author working on rewrite, slower development
- **Complexity**: LSM tree complexity can cause unpredictable latency
- **Memory Usage**: Can be high during compaction
- **Eventual Consistency**: Some operations not immediately durable

**Performance Profile**:
- Read: ~1-10μs depending on LSM level
- Write: ~5-20μs, can spike during compaction
- Range queries: Good, but can be affected by LSM structure
- Memory usage: Variable, depends on compaction state

**Verdict**: **Solid choice** - Proven reliability, but maintenance uncertainty

### 4. RocksDB (via rust-rocksdb)

**Architecture**: LSM tree with advanced compaction strategies

```rust
// RocksDB usage pattern
use rocksdb::{DB, Options, WriteBatch, IteratorMode};

let mut opts = Options::default();
opts.create_if_missing(true);
let db = DB::open(&opts, path)?;

// Write batch for transactions
let mut batch = WriteBatch::default();
batch.put(b"key1", b"value1");
batch.put(b"key2", b"value2");
db.write(batch)?;

// Range iteration
for (key, value) in db.iterator(IteratorMode::From(b"prefix", Direction::Forward)) {
    // Process entries
}
```

**Pros**:
- **Production-Proven**: Used by major databases and systems
- **High Performance**: Optimized for large-scale workloads
- **Rich Features**: Compression, bloom filters, column families
- **Tuneable**: Extensive configuration options
- **Stable**: Mature C++ codebase with Rust bindings

**Cons**:
- **C++ Dependency**: Not pure Rust, potential linking issues
- **Complex Configuration**: Many knobs to tune correctly
- **Large Binary**: Significant size overhead
- **API Complexity**: Low-level operations require deep understanding

**Performance Profile**:
- Read: ~500ns to 5μs depending on cache
- Write: ~1-50μs depending on batch size
- Range queries: Excellent with bloom filters
- Memory usage: Configurable, can be optimized

**Verdict**: **Overkill** - Great performance but too complex for embedded use

### 5. LMDB (via lmdb-rs)

**Architecture**: Memory-mapped B+ tree with copy-on-write

```rust
// LMDB usage pattern
use lmdb::{Environment, Database, Transaction, WriteFlags};

let env = Environment::new()
    .set_map_size(1_000_000_000) // 1GB
    .open(path)?;

let db = env.open_db(None)?;

// Write transaction
let mut txn = env.begin_rw_txn()?;
txn.put(db, &b"key", &b"value", WriteFlags::empty())?;
txn.commit()?;

// Read transaction
let txn = env.begin_ro_txn()?;
let value = txn.get(db, &b"key")?;
```

**Pros**:
- **ACID Compliant**: Strong consistency guarantees
- **Memory Mapped**: Efficient for large datasets
- **Multiple Readers**: Concurrent read access
- **Battle-Tested**: Used in many production systems
- **Copy-on-Write**: Good for snapshot semantics

**Cons**:
- **C Dependency**: Not pure Rust
- **Fixed Map Size**: Must pre-allocate maximum database size
- **Platform Differences**: Behavior varies between OSes
- **Write Amplification**: COW can cause performance issues

**Performance Profile**:
- Read: ~200ns for mmap cache hit
- Write: ~5-15μs for transaction commit
- Range queries: Excellent with B+ tree structure
- Memory usage: Low, memory-mapped

**Verdict**: **Good option** - Proven reliability, but C dependency is concerning

## Specialized Considerations

### For Version Control Workloads

**Pijul's Sanakirja Approach**:
- Uses content-addressed storage with cryptographic hashing
- Implements patch theory directly in storage layer
- Copy-on-write enables efficient branching
- Custom data structures for change dependencies

**Adapting to Filesystem VCS**:
- Store file content with SHA-256 addressing
- Use separate tables for: files, directories, metadata, content
- Implement efficient diff/patch storage
- Enable point-in-time filesystem reconstruction

### Performance Patterns for Filesystem Operations

```rust
// Example schema design for filesystem storage
pub struct FileSystemStorage {
    // Core tables
    files: Table<PathBuf, FileRecord>,
    directories: Table<PathBuf, DirectoryRecord>, 
    content: Table<ContentHash, Vec<u8>>,
    opcodes: Table<OpCodeId, OpCode>,
    
    // Index tables for fast queries
    path_to_opcodes: Table<PathBuf, Vec<OpCodeId>>,
    timestamp_index: Table<Timestamp, Vec<OpCodeId>>,
    content_refs: Table<ContentHash, Vec<PathBuf>>,
}

// Optimized for common filesystem queries:
// 1. Get file content: files[path] -> content_hash -> content[hash]
// 2. List directory: directories[path] -> entries
// 3. File history: path_to_opcodes[path] -> opcodes
// 4. Point-in-time: timestamp_index[time] -> opcodes -> reconstruct
```

## Recommendations

### Primary Choice: Sanakirja + Custom Layer

**Strategy**: Use Sanakirja as the foundation with a custom abstraction layer

```rust
// Proposed architecture
pub struct ClarisStorage {
    // Core Sanakirja environment
    env: sanakirja::Env,
    
    // Specialized tables
    file_table: Table<PathId, FileMetadata>,
    content_table: Table<ContentHash, ContentData>,
    opcode_table: Table<OpCodeId, OpCode>,
    
    // Indexes for fast queries
    path_index: Table<String, PathId>,
    history_index: Table<PathId, Vec<OpCodeId>>,
}

impl ClarisStorage {
    // High-level API for filesystem operations
    pub fn store_file_operation(&mut self, path: &Path, opcode: OpCode) -> Result<OpCodeId>;
    pub fn get_file_history(&self, path: &Path) -> Result<Vec<OpCode>>;
    pub fn get_file_at_time(&self, path: &Path, timestamp: u64) -> Result<Option<FileContent>>;
    pub fn create_snapshot(&self) -> Result<SnapshotId>;
    pub fn fork_from_snapshot(&self, snapshot: SnapshotId) -> Result<Self>;
}
```

**Benefits**:
- Proven in Pijul for version control use cases
- Copy-on-write semantics perfect for filesystem versioning
- Fork operation enables efficient branching
- Low-level control for optimization
- Pure Rust implementation

**Implementation Plan**:
1. Extract Sanakirja from Pijul repository
2. Create high-level abstraction layer
3. Implement filesystem-specific optimizations
4. Add comprehensive test suite
5. Performance tuning and benchmarking

### Fallback Choice: ReDB + Migration Path

**Strategy**: Start with ReDB for rapid development, migrate to Sanakirja later

```rust
// Dual implementation for gradual migration
pub enum StorageBackend {
    ReDB(ReDBStorage),
    Sanakirja(SanakirjaStorage),
}

pub trait FileSystemStorage {
    fn store_opcode(&mut self, opcode: OpCode) -> Result<OpCodeId>;
    fn get_file_history(&self, path: &Path) -> Result<Vec<OpCode>>;
    // ... other methods
}

impl FileSystemStorage for StorageBackend {
    fn store_opcode(&mut self, opcode: OpCode) -> Result<OpCodeId> {
        match self {
            StorageBackend::ReDB(storage) => storage.store_opcode(opcode),
            StorageBackend::Sanakirja(storage) => storage.store_opcode(opcode),
        }
    }
}
```

**Benefits**:
- Fast initial development with ReDB's simple API
- Performance validation early in development
- Smooth migration path to Sanakirja
- Risk mitigation if Sanakirja integration proves difficult

## Implementation Timeline

### Phase 1 (Week 1-2): ReDB Prototype
- Basic CRUD operations for OpCodes
- Simple file history queries
- Performance baseline establishment

### Phase 2 (Week 3-4): Sanakirja Extraction
- Extract and adapt Sanakirja from Pijul
- Create abstraction layer
- Port prototype operations

### Phase 3 (Week 5-6): Advanced Features
- Implement snapshot/fork operations
- Add compression and deduplication
- Performance optimization

### Phase 4 (Week 7-8): Production Readiness
- Comprehensive test suite
- Error handling and recovery
- Documentation and examples

This approach provides a solid foundation for high-performance filesystem version control while maintaining development velocity and reducing risk.