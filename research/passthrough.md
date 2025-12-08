# FUSE Passthrough and OverlayFS Research

## Overview

This document analyzes kernel-level passthrough mechanisms that could inform Ize's architecture for efficient filesystem operations with versioning.

---

## FUSE Passthrough

### What is FUSE Passthrough?

FUSE passthrough is a kernel feature (requires `CONFIG_FUSE_PASSTHROUGH`) that allows certain I/O operations on FUSE files to bypass the userspace daemon entirely, going directly to an underlying "backing file" on a lower filesystem.

**Key Insight**: This eliminates the kernel→userspace→kernel round-trip for read/write operations on files that don't need transformation.

### How It Works

```
Without Passthrough:
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│ Application │────▶│ FUSE Kernel  │────▶│ FUSE Daemon │
└─────────────┘     └──────────────┘     └──────────────┘
                           │                    │
                           │◀───────────────────┘
                           ▼
                    ┌─────────────┐
                    │  Real File  │
                    └─────────────┘

With Passthrough:
┌─────────────┐     ┌──────────────┐
│ Application │────▶│ FUSE Kernel  │────▶ Real File (direct)
└─────────────┘     └──────────────┘
                    (FUSE Daemon bypassed for I/O)
```

### Enabling Passthrough

1. **Compile-time**: `CONFIG_FUSE_PASSTHROUGH` must be enabled
2. **FUSE_INIT handshake**: Daemon negotiates `FUSE_PASSTHROUGH` capability and specifies `max_stack_depth`
3. **Register backing file**: Daemon uses `FUSE_DEV_IOC_BACKING_OPEN` ioctl to register a backing file descriptor, receiving a `backing_id`
4. **Open response**: When handling `OPEN`/`CREATE`, daemon replies with `FOPEN_PASSTHROUGH` flag and `backing_id`
5. **Cleanup**: `FUSE_DEV_IOC_BACKING_CLOSE` releases kernel's reference

### Supported Operations

- `read(2)` / `write(2)` (via `read_iter` / `write_iter`)
- `splice(2)`
- `mmap(2)`

### Privilege Requirements

**Currently requires `CAP_SYS_ADMIN`** due to:

1. **Resource Accounting**: After daemon closes its FD, kernel still holds reference via `struct fuse_backing`. This:
   - Makes the file invisible to `lsof` and other inspection tools
   - Bypasses `RLIMIT_NOFILE` limits (potential DoS vector)

2. **Filesystem Stacking Loops**: Complex stacking scenarios (FUSE on FUSE, FUSE under OverlayFS, etc.) could create:
   - Dependency loops during shutdown
   - Deadlocks
   - Similar risks to `LOOP_SET_FD` (also requires `CAP_SYS_ADMIN`)

3. **Stack Depth Limits**: Kernel checks `sb->s_stack_depth` and `fc->max_stack_depth` to prevent excessive nesting

### Relevance to Ize

**Direct applicability**: For files that don't need content transformation (just tracking), passthrough could provide:
- Near-native I/O performance
- Reduced latency
- Lower CPU overhead

**Limitation**: Requires kernel 5.x+ with `CONFIG_FUSE_PASSTHROUGH` and `CAP_SYS_ADMIN`. Not universally available.

**Alternative for Ize**: Even without kernel passthrough, we can use:
- Direct FD access via `openat()` (see shadowing solution)
- Asynchronous operation recording (don't block I/O for versioning)

---

## OverlayFS

### What is OverlayFS?

OverlayFS is a kernel filesystem that overlays one directory tree on top of another, presenting a merged view. It's the foundation for container filesystems (Docker, etc.).

### Core Concepts

```
                    ┌─────────────┐
                    │   Merged    │  ← User sees this
                    │    View     │
                    └──────┬──────┘
                           │
           ┌───────────────┼───────────────┐
           │               │               │
    ┌──────▼──────┐ ┌──────▼──────┐ ┌──────▼──────┐
    │   Upper     │ │   Lower 1   │ │   Lower 2   │
    │ (writable)  │ │ (read-only) │ │ (read-only) │
    └─────────────┘ └─────────────┘ └─────────────┘
```

### Key Features

#### 1. Upper and Lower Layers

- **Upper**: Writable layer where modifications go
- **Lower**: Read-only layer(s) providing base content
- **Merged**: The visible overlay mount

```bash
mount -t overlay overlay \
    -o lowerdir=/lower,upperdir=/upper,workdir=/work \
    /merged
```

#### 2. Copy-Up on Write

When modifying a file from lower layer:
1. File is copied from lower to upper (copy-up)
2. Modifications happen in upper
3. Original lower file unchanged

```
Before write:
/lower/file.txt  →  /merged/file.txt (read-only view)

After write:
/lower/file.txt  (unchanged)
/upper/file.txt  (modified copy)
/merged/file.txt →  /upper/file.txt
```

#### 3. Whiteouts and Opaque Directories

**Whiteouts**: Mark deleted files without modifying lower layer
- Character device with 0/0 device number, OR
- Zero-size file with `trusted.overlay.whiteout` xattr

**Opaque directories**: Mark directories that hide lower contents
- Set `trusted.overlay.opaque` xattr to "y"

#### 4. Metacopy (Metadata-only Copy-up)

For metadata changes (chmod, chown), only metadata is copied:
- Upper file marked with `trusted.overlayfs.metacopy` xattr
- Data remains in lower until actual data write
- Saves space and I/O for metadata-only operations

#### 5. Directory Merging

Directories from upper and lower are merged:
- readdir returns combined entries
- Upper entries shadow lower entries with same name

#### 6. Data-only Lower Layers

Since kernel 6.8, layers can be "data-only" (specified with `::`):
```bash
mount -t overlay overlay \
    -o lowerdir=/meta1:/meta2::/data1::/data2 \
    /merged
```
Data-only layers provide content for metacopy files but aren't visible in directory listings.

### OverlayFS Patterns Relevant to Ize

#### Pattern 1: Layered Storage

```
Ize could use a similar model:

/pristine/          ← "Lower" - immutable versioned snapshots
    snapshot-001/
    snapshot-002/
    
/working/           ← "Upper" - current working state
    
/mounted/           ← Merged view users interact with
```

#### Pattern 2: Copy-on-Write Semantics

OverlayFS's copy-up is similar to what Ize needs:
- Track when files diverge from known state
- Store deltas/changes separately
- Original content preserved

#### Pattern 3: Whiteout Concept for Deletions

For versioning, "whiteouts" could represent:
- File deletion events
- Tombstones in version history
- Efficient deletion tracking without data removal

#### Pattern 4: Index Directory

OverlayFS uses index for:
- NFS file handle verification
- Hard link tracking across layers
- Origin verification

Ize could use similar indexing for:
- Content-addressed deduplication
- Version ancestry tracking
- Inode stability

### Important Constraints

1. **Changes to underlying filesystems while mounted are not allowed**
   - Behavior is undefined (but no crash/deadlock)
   - Similar constraint for Ize: source dir should only be accessed through mount

2. **Stacking limits**
   - Max depth tracked via `s_stack_depth`
   - Prevents infinite recursion

3. **st_dev / st_ino behavior**
   - Non-directories may report different st_dev
   - Can be unified with `xino` feature
   - Ize needs consistent inode presentation

---

## Comparison: FUSE Passthrough vs OverlayFS

| Feature | FUSE Passthrough | OverlayFS |
|---------|------------------|-----------|
| **Location** | Userspace daemon with kernel bypass | Purely kernel |
| **Flexibility** | Full control in daemon | Fixed overlay semantics |
| **Performance** | Near-native (with passthrough) | Native kernel speed |
| **Privileges** | `CAP_SYS_ADMIN` for passthrough | Root for trusted xattrs |
| **Write model** | Application-defined | Copy-on-write to upper |
| **Versioning** | Application responsibility | Layers are versions |
| **Use case** | Custom filesystem logic | Container layers |

---

## Implications for Ize Architecture

### Option 1: Pure FUSE (Current Approach)

```
┌─────────────────────────────────────────┐
│              Application                │
└─────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────┐
│           FUSE (Ize Daemon)             │
│  • Intercepts all operations            │
│  • Records to Sanakirja DB              │
│  • Passes through to source dir         │
└─────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
┌───────────────┐       ┌───────────────┐
│  Source Dir   │       │   Sanakirja   │
│  (real files) │       │  (versions)   │
└───────────────┘       └───────────────┘
```

**Pros**: Full control, portable
**Cons**: All I/O goes through daemon

### Option 2: FUSE with Kernel Passthrough

```
┌─────────────────────────────────────────┐
│              Application                │
└─────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────┐
│              FUSE Kernel                │
│  • Metadata ops → Ize Daemon            │
│  • Data I/O → Direct passthrough        │
└─────────────────────────────────────────┘
           │                    │
           ▼                    ▼
┌───────────────┐       ┌───────────────┐
│  Ize Daemon   │       │  Source Dir   │
│  (versioning) │       │ (passthrough) │
└───────────────┘       └───────────────┘
```

**Pros**: Near-native data I/O performance
**Cons**: Requires CAP_SYS_ADMIN, kernel support, complex setup

### Option 3: OverlayFS-Inspired Design

```
┌─────────────────────────────────────────┐
│           Application                   │
└─────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────┐
│         FUSE (Ize Daemon)               │
│  • Maintains "snapshot" lower layers    │
│  • Tracks modifications in "upper"      │
│  • Presents merged view                 │
└─────────────────────────────────────────┘
           │                    │
           ▼                    ▼
┌───────────────┐       ┌───────────────┐
│   Snapshots   │       │   Working     │
│   (Sanakirja) │       │   (current)   │
└───────────────┘       └───────────────┘
```

**Pros**: Clean versioning model, familiar semantics
**Cons**: More complex implementation, potential storage overhead

### Recommended Approach for Ize

**Hybrid Strategy**:

1. **Phase 1**: Pure FUSE with FD preservation
   - Use `openat()` with preserved directory FD
   - Bypass FUSE for source file access (no kernel passthrough needed)
   - Async versioning to minimize latency

2. **Phase 2**: Optional kernel passthrough
   - When `CONFIG_FUSE_PASSTHROUGH` available and `CAP_SYS_ADMIN` granted
   - Use for read-heavy workloads
   - Fall back to Phase 1 approach otherwise

3. **Borrow from OverlayFS**:
   - Whiteout concept for deletion tracking
   - Metacopy pattern (version metadata separately from content)
   - Index directory for content-addressed storage

---

## Technical Details: FD-Based Source Access

For the shadowing problem (mounting FUSE at/near source directory), use file descriptor preservation:

```rust
use std::os::unix::io::{AsRawFd, OwnedFd, FromRawFd};
use libc::{openat, open, O_PATH, O_DIRECTORY, O_RDONLY, O_WRONLY};

pub struct IzeFuse {
    /// FD to source directory, opened BEFORE mounting FUSE
    source_dir_fd: OwnedFd,
}

impl IzeFuse {
    pub fn new(source_dir: &Path) -> io::Result<Self> {
        // Open source directory with O_PATH before mounting
        // This gives us a handle that bypasses VFS path resolution
        let fd = unsafe {
            let path = CString::new(source_dir.as_os_str().as_bytes())?;
            let fd = open(path.as_ptr(), O_PATH | O_DIRECTORY);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };
        
        Ok(Self { source_dir_fd: fd })
    }
    
    /// Open a file in source directory, bypassing FUSE mount
    pub fn open_source_file(&self, relative_path: &Path, flags: i32) -> io::Result<File> {
        let path = CString::new(relative_path.as_os_str().as_bytes())?;
        let fd = unsafe {
            openat(
                self.source_dir_fd.as_raw_fd(),
                path.as_ptr(),
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

**Why this works**: `openat()` with a directory FD resolves paths relative to that FD, completely bypassing VFS path resolution (and thus FUSE) for the directory lookup portion.

---

## References

- [FUSE Passthrough Documentation](https://docs.kernel.org/next/filesystems/fuse-passthrough.html)
- [LWN: FUSE passthrough for file I/O (2023)](https://lwn.net/Articles/932060/)
- [OverlayFS Documentation](https://docs.kernel.org/filesystems/overlayfs.html)
- [fuse-bpf: Alternative approach](https://lwn.net/Articles/933959/)

---

## Appendix: OverlayFS Mount Options Reference

| Option | Description |
|--------|-------------|
| `lowerdir=` | Colon-separated lower directories (right to left stacking) |
| `upperdir=` | Writable upper directory |
| `workdir=` | Work directory (same fs as upper) |
| `redirect_dir=on/off` | Enable directory redirects for rename |
| `index=on/off` | Enable inode index for hard links |
| `metacopy=on/off` | Enable metadata-only copy-up |
| `xino=on/off/auto` | Extended inode numbers for unique st_ino |
| `volatile` | Skip sync calls (data loss risk) |
| `userxattr` | Use user.overlay.* instead of trusted.overlay.* |