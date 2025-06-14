# Claris-FUSE Architecture

## Overview

Claris-FUSE is a version-controlled filesystem implemented with FUSE in Rust. It maintains a linear history of file operations (create/update/delete) similar to Git but at the filesystem level, tracking changes to files over time and allowing users to view and restore previous versions.

## Core Philosophy

- **Transparent Versioning**: Every filesystem operation is tracked without user intervention
- **Performance First**: Asynchronous operation queue ensures filesystem performance isn't impacted
- **Pluggable Storage**: Clean trait-based architecture allows swapping storage backends
- **Mathematical Rigor**: Inspired by Pijul's approach to version control theory

## Technical Stack

### Foundation
- **Rust**: Systems programming language for safety and performance
- **Fuser (v0.15.1)**: Maintained FUSE bindings for Rust
  - Active maintenance and modern FUSE ABI support
  - File descriptor passthrough functionality
  - Superior to deprecated fuse-rs

### Storage Backend (Current)
- **SQLite**: Initial implementation for simplicity
- **Sanakirja** (Planned): Copy-on-write B+ tree engine from Pijul
  - Native support for snapshots and forks
  - Memory-mapped architecture for performance
  - Perfect fit for filesystem versioning needs

### Architecture Components
- **Op Queue**: Asynchronous operation processing (renamed from Command Queue)
- **PassthroughFS**: Transparent filesystem layer
- **Storage Trait**: Pluggable backend interface
- **CLI**: User interaction layer

## System Architecture

### Layer Overview

```
┌─────────────────────────────────────────┐
│          CLI Interface                  │
├─────────────────────────────────────────┤
│        VersionedFS Layer                │  ← Intercepts operations
├─────────────────────────────────────────┤
│       PassthroughFS Layer               │  ← Transparent operations
├─────────────────────────────────────────┤
│           Op Queue                      │  ← Async processing
├─────────────────────────────────────────┤
│     Storage Trait Interface             │  ← Pluggable backends
├─────────────────────────────────────────┤
│   SQLite │ Sanakirja │ Custom          │  ← Storage implementations
└─────────────────────────────────────────┘
```

### PassthroughFS Layer

Provides transparent filesystem operations with:
- Path management and normalization
- Inode allocation and tracking
- Read-only mode support
- Database file hiding from mount view
- Full POSIX compliance

### Op Queue System

Asynchronous operation processing for performance:

```rust
pub enum OpType {
    Create,
    Write { offset: i64, data: Vec<u8> },
    Delete,
    Rename { old_path: String, new_path: String },
    Truncate { size: u64 },
    SetAttr { attr: FileAttr },
    // Directory operations
    MakeDir,
    RemoveDir,
}

pub struct Op {
    id: Option<u64>,
    op_type: OpType,
    timestamp: u64,
    path: String,
    metadata: Option<serde_json::Value>,
}
```

### Storage Trait Interface

```rust
pub trait Storage: Send + Sync {
    fn init(path: &Path) -> Result<Self>;
    fn store_op(&mut self, op: &Op) -> Result<()>;
    fn get_ops_for_path(&self, path: &str) -> Result<Vec<Op>>;
    fn get_file_at_version(&self, path: &str, version: u64) -> Result<FileContent>;
    fn create_snapshot(&self) -> Result<SnapshotId>;
}
```

## Database Schema

### Core Entities

#### Operations Table
```sql
CREATE TABLE operations (
    id INTEGER PRIMARY KEY,
    op_type TEXT NOT NULL,
    path TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    parent_op_id INTEGER,
    metadata TEXT,
    content_id INTEGER,
    FOREIGN KEY (content_id) REFERENCES content(id)
);
```

#### Directories Table
```sql
CREATE TABLE directories (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    created_at INTEGER NOT NULL,
    metadata_id INTEGER,
    FOREIGN KEY (metadata_id) REFERENCES metadata(id)
);
```

#### Files Table
```sql
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    directory_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    metadata_id INTEGER,
    FOREIGN KEY (directory_id) REFERENCES directories(id),
    FOREIGN KEY (metadata_id) REFERENCES metadata(id)
);
```

#### Metadata Table
```sql
CREATE TABLE metadata (
    id INTEGER PRIMARY KEY,
    mode INTEGER NOT NULL,
    uid INTEGER NOT NULL,
    gid INTEGER NOT NULL,
    atime INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    ctime INTEGER NOT NULL
);
```

#### Content Table
```sql
CREATE TABLE content (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    data BLOB NOT NULL,
    FOREIGN KEY (file_id) REFERENCES files(id)
);
```

## Filesystem Operations

### Core Operations (Version Tracked)
1. **create** - New file creation
2. **write** - File content modification
3. **unlink** - File deletion
4. **rename** - File/directory moving
5. **truncate** - File size changes
6. **mkdir** - Directory creation
7. **rmdir** - Directory removal
8. **symlink** - Symbolic link creation
9. **link** - Hard link creation
10. **setattr** - Attribute modification

### Metadata Operations
- **chmod** - Permission changes
- **chown** - Ownership changes
- **utimens** - Timestamp updates
- **setxattr/removexattr** - Extended attributes

### Read-Only Operations
- **lookup**, **getattr**, **open**, **read**
- **readdir**, **readlink**, **access**
- **getxattr**, **listxattr**
- **flush**, **fsync**, **release**

## Implementation Roadmap

### Phase 1: Testing Framework ✓
- Clean test harness architecture
- Property-based testing for invariants
- Comprehensive test coverage

### Phase 2: Performance Benchmarking
- Operation throughput analysis
- Storage backend comparisons
- Regression detection framework

### Phase 3: Op Queue Refactoring
- Command → Op renaming
- Improved async processing
- Batch optimization

### Phase 4: Sanakirja Integration
- Extract from Pijul codebase
- Adapt for filesystem operations
- Implement snapshot/fork operations

### Phase 5: Advanced Features
- Delta compression
- Semantic change descriptions (LLM)
- Configurable retention policies

## Usage

### Initialization
```bash
claris-fuse init /path/to/directory
```

### Mounting
```bash
# Standard mount
claris-fuse mount /initialized/directory /mount/point

# Read-only mount
claris-fuse mount --read-only /initialized/directory /mount/point
```

### History Operations (Planned)
```bash
# View file history
claris-fuse history /mount/point/file.txt

# Restore specific version
claris-fuse restore /mount/point/file.txt --version=3

# Create snapshot
claris-fuse snapshot create --name="before-refactor"
```

## Development Practices

### Code Organization
- `/src/filesystem/` - FUSE filesystem implementation
- `/src/storage/` - Storage backend implementations
- `/src/op/` - Operation queue system
- `/src/cli/` - Command-line interface
- `/tests/` - Test harness and test suites
- `/research/` - Design documents and analysis

### Quality Standards
- All commits pass Clippy lints
- Comprehensive test coverage
- Benchmarks for performance-critical paths
- Clear documentation for public APIs

### Contribution Guidelines
- Atomic commits with clear messages
- Property-based tests for new features
- Benchmarks for performance changes
- Update architecture docs for design changes

## Future Enhancements

### Voice Integration
- Natural language filesystem operations
- Integration with Claris Mobile
- Voice-driven version control

### Distributed Features
- Multi-device synchronization
- Conflict-free replicated data types (CRDTs)
- Peer-to-peer filesystem sharing

### AI-Powered Features
- Automatic change descriptions
- Smart file organization suggestions
- Predictive caching based on usage patterns

## References

- [Pijul Theory](https://pijul.org/manual/theory.html) - Patch theory inspiration
- [FUSE Documentation](https://libfuse.github.io/doxygen/) - Filesystem interface
- [Sanakirja](https://docs.rs/sanakirja/) - Copy-on-write B+ trees