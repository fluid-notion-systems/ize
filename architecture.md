# Ize Architecture

## Overview

Ize is a version-controlled filesystem implemented with FUSE in Rust. It maintains a linear history of file operations (create/update/delete) similar to Git but at the filesystem level, tracking changes to files over time and allowing users to view and restore previous versions.

The system works by maintaining Pijul repositories as the source of truth, with FUSE mounting providing transparent access and automatic versioning of all filesystem operations.

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

### Storage Backend
- **Pijul/Sanakirja**: Copy-on-write B+ tree engine from Pijul
  - Native support for snapshots and forks
  - Memory-mapped architecture for performance
  - ACID transactions for crash safety
  - Content-addressed storage for deduplication
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
│     Pijul/Sanakirja Backend             │  ← Version control storage
└─────────────────────────────────────────┘
```

### Directory Layout

Ize uses a centralized storage approach with separate mount points:

```
Central Ize Directory:
~/.local/share/ize/
└── projects/
    ├── {project-uuid-1}/
    │   ├── .pijul/              # Pijul working tree
    │   │   ├── pristine/        # Sanakirja database
    │   │   ├── changes/         # Patch storage
    │   │   └── config           # Pijul configuration
    │   └── working/             # Actual file content
    │       ├── src/
    │       └── ...
    └── {project-uuid-2}/
        └── ...

Mount Point (User-specified):
/any/path/user/chooses/
├── src/                         # FUSE-mounted view
├── file.txt                     # Direct access to Pijul working tree
└── .ize/                        # HIDDEN from directory listings
    └── config                   # Points to central storage
```

**Key Characteristics:**
- **Central Storage**: All Pijul repositories live in `~/.local/share/ize/`
- **Pijul Working Trees**: Each project is a full Pijul repository with working directory
- **Mount Points**: FUSE mounts provide transparent access to the working trees
- **File Descriptor Access**: Pre-mount FD preservation allows FUSE to access source files even when mounted
- **Multiple Branches**: Future support for concurrent branch checkouts (noted but not yet implemented)

### PassthroughFS Layer

Provides transparent filesystem operations with:
- Path management and normalization
- Inode allocation and tracking
- Read-only mode support
- `.ize/` directory hiding from mount view
- File descriptor-based access to prevent shadowing
- Full POSIX compliance

**Shadowing Prevention**: The filesystem opens the Pijul working directory before mounting and preserves the file descriptor. All file operations use `openat()` with this preserved FD, allowing the FUSE layer to access the real files even when mounted at the same location (overlay mounting).

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
pub struct IzeDB {
    env: sanakirja::Env,
}

/// Core operations
impl IzeDB {
    fn record_operation(&mut self, op: &Op) -> Result<()>;
    fn create_snapshot(&self, name: &str) -> Result<SnapshotId>;
    fn get_file_at_op(&self, path: &str, op_id: u64) -> Result<FileContent>;
}
```

## Sanakirja Schema Design

Ize uses Sanakirja (Pijul's B+ tree database) with content-addressed storage:

### Database Structure

```rust
/// Database tables (B+ trees)
impl IzeDB {
    const ROOT_FILES: usize = 0;      // PathId -> FileRecord
    const ROOT_DIRS: usize = 1;       // PathId -> DirRecord  
    const ROOT_CONTENT: usize = 2;    // ContentHash -> offset
    const ROOT_OPERATIONS: usize = 3; // OpId -> Operation
    const ROOT_PATH_INDEX: usize = 4; // String -> PathId
    const ROOT_HISTORY: usize = 5;    // PathId -> Vec<OpId>
}

#[repr(C)]
pub struct FileRecord {
    pub path_id: u64,
    pub content_hash: [u8; 32],  // SHA-256
    pub size: u64,
    pub mode: u32,
    pub mtime: u64,
    pub last_op_id: u64,
}

#[repr(C)]
pub struct OpRecord {
    pub op_id: u64,
    pub op_type: u8,
    pub path_id: u64,
    pub timestamp: u64,
    pub prev_content_hash: [u8; 32],
    pub new_content_hash: [u8; 32],
    pub metadata: u64,
}
```

### Storage Layout

```
~/.local/share/ize/projects/{project-uuid}/
├── .pijul/
│   ├── pristine/
│   │   └── db              # Sanakirja B+ tree database
│   ├── changes/            # Pijul patches (future use)
│   └── config              # Pijul configuration
├── working/                # Pijul working directory
│   └── ...                 # Actual files
└── content/                # Content-addressed storage
    └── ab/
        └── cdef123...      # SHA-256 addressed blobs
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

### Phase 4: Pijul Integration ✓
- Use Pijul as the storage backend
- Sanakirja database for metadata and operations
- Content-addressed storage for file data
- File descriptor preservation for overlay mounting

### Phase 5: Advanced Features
- Delta compression
- Semantic change descriptions (LLM)
- Configurable retention policies

## Usage

### Initialization
```bash
# Initialize a new Ize-tracked project
ize init my-project

# This creates:
# ~/.local/share/ize/projects/{uuid}/
#   ├── .pijul/           # Pijul repository
#   └── working/          # Working directory
```

### Mounting
```bash
# Mount to a specific location
ize mount my-project /path/to/mount/point

# Read-only mount
ize mount --read-only my-project /path/to/mount/point

# Overlay mount (mounts at same location as working directory)
ize mount --overlay my-project
```

**Note on Overlay Mounting**: When using `--overlay`, Ize mounts at the same location as the Pijul working directory. This is possible because Ize opens the working directory and preserves its file descriptor before mounting, then uses `openat()` to access the real files, bypassing the FUSE layer.

### History Operations (Planned)
```bash
# View file history
ize history /mount/point/file.txt

# Restore specific version
ize restore /mount/point/file.txt --version=3

# Create snapshot
ize snapshot create --name="before-refactor"

# List projects
ize list

# Show project info
ize info my-project
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
- Voice-driven version control

### Distributed Features
- Multi-device synchronization via Pijul
- Pijul's patch-based merging for conflict resolution
- Peer-to-peer filesystem sharing through Pijul remotes

### Multiple Branch Support
- Concurrent checkouts of different branches (planned)
- Each branch as a separate working tree in central storage
- Fast branch switching without file copying

### AI-Powered Features
- Automatic change descriptions
- Smart file organization suggestions
- Predictive caching based on usage patterns

## Technical Notes

### Shadowing Prevention

When FUSE mounts at a directory, it "shadows" the original contents, making them inaccessible through normal path resolution. Ize solves this through **file descriptor preservation**:

1. Open the Pijul working directory before mounting (`O_PATH | O_DIRECTORY`)
2. Store the file descriptor in the FUSE filesystem struct
3. Use `openat()` with the preserved FD for all file operations
4. `openat()` resolves paths relative to the FD, bypassing the VFS layer and FUSE

This technique allows overlay mounting where the mount point and source directory are the same location.

### Differences from Pijul

While Ize uses Pijul/Sanakirja as storage, it differs in key ways:

| Aspect | Pijul | Ize |
|--------|-------|-----|
| **Interface** | CLI commands | FUSE filesystem |
| **Tracking** | Explicit `pijul record` | Automatic on write |
| **Granularity** | Line-level patches | Byte-level operations |
| **Primary Unit** | Patch (change) | Operation (event) |
| **Working Copy** | Regular files | FUSE-intercepted files |

**What Ize takes from Pijul:**
- Sanakirja storage engine for transactions and snapshots
- Content-addressing for deduplication
- Copy-on-write for efficient versioning
- Distributed collaboration infrastructure (future)

**What Ize doesn't use (yet):**
- Patch theory / commutative changes
- Line-level diff tracking
- Explicit recording workflow
- Complex merge/conflict resolution

## References

- [Pijul Theory](https://pijul.org/manual/theory.html) - Patch theory inspiration
- [FUSE Documentation](https://libfuse.github.io/doxygen/) - Filesystem interface
- [Sanakirja](https://docs.rs/sanakirja/) - Copy-on-write B+ trees
- [Pijul Backend Migration Analysis](research/pijul_backend_migration.md) - Detailed migration design
