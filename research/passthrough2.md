# PassthroughFS2 Design Document

## Overview

This document outlines the architecture for `PassthroughFS2`, a FUSE passthrough filesystem that transparently maps operations between a **source directory** and a **mount point**. Unlike the original `PassthroughFS`, this implementation:

1. Has no concept of a "database file" - just `source_dir` ↔ `mount_point`
2. Uses **real inodes** from the underlying filesystem
3. Uses **real file descriptors** as FUSE file handles
4. Implements proper file descriptor lifecycle management

## Key Insight: Use Real Values

### Real Inodes

Instead of maintaining synthetic inode numbers, we use the actual inode from the source filesystem:

```rust
fn get_inode(real_path: &Path) -> io::Result<u64> {
    let metadata = fs::metadata(real_path)?;
    Ok(metadata.ino())
}
```

Benefits:
- No memory overhead for inode tracking
- Hard links work correctly (same inode)
- No state to get out of sync
- Inodes are stable across renames (within same filesystem)

**Caveat**: We still need inode → path reverse lookup for operations like `getattr(ino)`.

### Real File Descriptors as Handles

The `fh` (file handle) in FUSE is just a `u64` we return from `open()`. We use the **actual fd** as this handle:

```rust
fn open(&mut self, ..., reply: ReplyOpen) {
    let file = File::open(&real_path)?;
    let fd = file.as_raw_fd();  // e.g., fd = 7
    
    // Store File to keep fd alive
    self.file_handles.insert(fd, file);
    
    reply.opened(fd as u64, 0);  // fh = 7 (same as fd)
}

fn read(&mut self, ..., fh: u64, ...) {
    // fh IS the fd
    let fd = fh as i32;  // fd = 7
    pread(fd, ...);      // Works because File is still in table
}

fn release(&mut self, ..., fh: u64, ...) {
    let fd = fh as i32;
    self.file_handles.remove(&fd);  // File dropped → fd closed via RAII
}
```

## Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          PassthroughFS2                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  source_dir: PathBuf        ← The real directory being exposed              │
│  mount_point: PathBuf       ← Where it's mounted (for reference)            │
│  read_only: bool            ← Mount mode                                    │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                     inode_to_path: HashMap<u64, PathBuf>             │   │
│  │                                                                       │   │
│  │  Maps real inode → relative path                                     │   │
│  │  Populated during lookup() and readdir()                             │   │
│  │  Used for inode-based operations (getattr, setattr, etc.)            │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                     file_handles: HashMap<i32, FileHandle>           │   │
│  │                                                                       │   │
│  │  Key = real fd (also used as FUSE fh)                                │   │
│  │  Value = FileHandle { file: File, path: PathBuf, flags: i32 }        │   │
│  │  Keeps File alive so fd remains valid                                │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Data Structures

```rust
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::RwLock;

/// Stored file handle - keeps the File alive so fd remains valid
struct FileHandle {
    /// The File object that owns the fd
    file: File,
    /// The real path (useful for some operations)
    real_path: PathBuf,
    /// Flags used when opening (O_RDONLY, O_RDWR, etc.)
    flags: i32,
}

pub struct PassthroughFS2 {
    /// The source directory being exposed
    source_dir: PathBuf,
    /// The mount point
    mount_point: PathBuf,
    /// Read-only mode
    read_only: bool,
    
    /// Maps real inode → relative path (for inode-based lookups)
    inode_to_path: RwLock<HashMap<u64, PathBuf>>,
    
    /// Maps fd → FileHandle (keeps File alive, fd is also the FUSE fh)
    file_handles: RwLock<HashMap<i32, FileHandle>>,
}
```

### Path Helpers

Simple path conversion - no complex PathManager needed:

```rust
impl PassthroughFS2 {
    /// Convert relative path to real path in source_dir
    fn to_real(&self, rel_path: &Path) -> PathBuf {
        self.source_dir.join(rel_path)
    }
    
    /// Get inode for a path (uses real inode from filesystem)
    fn get_inode(&self, real_path: &Path) -> io::Result<u64> {
        Ok(fs::metadata(real_path)?.ino())
    }
    
    /// Register an inode → path mapping
    fn register_inode(&self, ino: u64, rel_path: PathBuf) {
        self.inode_to_path.write().unwrap().insert(ino, rel_path);
    }
    
    /// Look up path for an inode
    fn get_path_for_inode(&self, ino: u64) -> Option<PathBuf> {
        self.inode_to_path.read().unwrap().get(&ino).cloned()
    }
}
```

## Data Flow

```
User Process                    FUSE Kernel            PassthroughFS2
    │                               │                       │
    │ open("/mnt/foo.txt", O_RDWR)  │                       │
    │ ─────────────────────────────>│                       │
    │                               │ open(ino=12345, O_RDWR)
    │                               │ ─────────────────────>│
    │                               │                       │ 1. inode_to_path[12345]
    │                               │                       │    → "foo.txt"
    │                               │                       │ 2. to_real("foo.txt")
    │                               │                       │    → "/source/foo.txt"
    │                               │                       │ 3. File::open(...) → File
    │                               │                       │ 4. fd = file.as_raw_fd() → 7
    │                               │                       │ 5. file_handles[7] = FileHandle{file, ...}
    │                               │ reply.opened(fh=7)    │
    │                               │ <─────────────────────│
    │ fd=3                          │                       │
    │ <─────────────────────────────│                       │
    │                               │                       │
    │ write(fd=3, "hello", 5)       │                       │
    │ ─────────────────────────────>│                       │
    │                               │ write(fh=7, "hello")  │
    │                               │ ─────────────────────>│
    │                               │                       │ fd = fh = 7
    │                               │                       │ pwrite(7, "hello", ...)
    │                               │ reply.written(5)      │
    │                               │ <─────────────────────│
    │ 5                             │                       │
    │ <─────────────────────────────│                       │
    │                               │                       │
    │ close(fd=3)                   │                       │
    │ ─────────────────────────────>│                       │
    │                               │ release(fh=7)         │
    │                               │ ─────────────────────>│
    │                               │                       │ file_handles.remove(7)
    │                               │                       │ // File dropped → fd 7 closed
    │                               │ reply.ok()            │
    │                               │ <─────────────────────│
    │ 0                             │                       │
    │ <─────────────────────────────│                       │
```

## Implementation

### open() - Store File, Return fd as Handle

```rust
fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
    // 1. Get path from inode
    let rel_path = match self.get_path_for_inode(ino) {
        Some(p) => p,
        None => { reply.error(ENOENT); return; }
    };
    let real_path = self.to_real(&rel_path);
    
    // 2. Open with appropriate flags
    let mut options = OpenOptions::new();
    match flags & libc::O_ACCMODE {
        libc::O_RDONLY => { options.read(true); }
        libc::O_WRONLY => { options.write(true); }
        libc::O_RDWR   => { options.read(true).write(true); }
        _ => { options.read(true); }
    }
    if flags & libc::O_APPEND != 0 { options.append(true); }
    if flags & libc::O_TRUNC != 0 { options.truncate(true); }
    
    match options.open(&real_path) {
        Ok(file) => {
            // 3. Get the real fd - this IS our file handle
            let fd = file.as_raw_fd();
            
            // 4. Store File to keep fd alive
            let handle = FileHandle {
                file,
                real_path,
                flags,
            };
            self.file_handles.write().unwrap().insert(fd, handle);
            
            // 5. Return fd as the FUSE file handle
            reply.opened(fd as u64, 0);
        }
        Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
    }
}
```

### read() - Use fd Directly

```rust
fn read(&mut self, _req: &Request, _ino: u64, fh: u64, offset: i64, 
        size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
    // fh IS the fd
    let fd = fh as i32;
    
    // Verify we have this handle (optional safety check)
    if !self.file_handles.read().unwrap().contains_key(&fd) {
        reply.error(libc::EBADF);
        return;
    }
    
    // Use pread for thread-safe positional read
    let mut buf = vec![0u8; size as usize];
    let n = unsafe {
        libc::pread(fd, buf.as_mut_ptr() as *mut libc::c_void, size as usize, offset)
    };
    
    if n >= 0 {
        buf.truncate(n as usize);
        reply.data(&buf);
    } else {
        reply.error(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
    }
}
```

### write() - Use fd Directly

```rust
fn write(&mut self, _req: &Request, _ino: u64, fh: u64, offset: i64, 
         data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, 
         reply: ReplyWrite) {
    let fd = fh as i32;
    
    if !self.file_handles.read().unwrap().contains_key(&fd) {
        reply.error(libc::EBADF);
        return;
    }
    
    // Use pwrite for thread-safe positional write
    let n = unsafe {
        libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset)
    };
    
    if n >= 0 {
        reply.written(n as u32);
    } else {
        reply.error(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
    }
}
```

### flush() - fsync the fd

```rust
fn flush(&mut self, _req: &Request, _ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
    let fd = fh as i32;
    
    if !self.file_handles.read().unwrap().contains_key(&fd) {
        reply.error(libc::EBADF);
        return;
    }
    
    if unsafe { libc::fsync(fd) } == 0 {
        reply.ok();
    } else {
        reply.error(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
    }
}
```

### release() - Remove from Table, fd Closes via RAII

```rust
fn release(&mut self, _req: &Request, _ino: u64, fh: u64, _flags: i32,
           _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
    let fd = fh as i32;
    
    // Remove from table - File is dropped, fd is automatically closed
    self.file_handles.write().unwrap().remove(&fd);
    
    reply.ok();
}
```

### setattr() with Proper Truncate

```rust
fn setattr(&mut self, _req: &Request, ino: u64, mode: Option<u32>, 
           uid: Option<u32>, gid: Option<u32>, size: Option<u64>,
           atime: Option<TimeOrNow>, mtime: Option<TimeOrNow>,
           _ctime: Option<SystemTime>, fh: Option<u64>, ..., reply: ReplyAttr) {
    
    let rel_path = match self.get_path_for_inode(ino) {
        Some(p) => p,
        None => { reply.error(ENOENT); return; }
    };
    let real_path = self.to_real(&rel_path);
    
    // Handle truncate
    if let Some(new_size) = size {
        let result = if let Some(fh_val) = fh {
            let fd = fh_val as i32;
            let handles = self.file_handles.read().unwrap();
            
            if let Some(handle) = handles.get(&fd) {
                // Check if opened for writing
                if (handle.flags & (libc::O_WRONLY | libc::O_RDWR)) != 0 {
                    // Can use ftruncate directly on the fd
                    if unsafe { libc::ftruncate(fd, new_size as i64) } == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                } else {
                    // Not opened for writing, fall back to path-based
                    drop(handles);
                    truncate_via_path(&real_path, new_size)
                }
            } else {
                drop(handles);
                truncate_via_path(&real_path, new_size)
            }
        } else {
            truncate_via_path(&real_path, new_size)
        };
        
        if let Err(e) = result {
            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            return;
        }
    }
    
    // Handle mode, uid, gid, times...
    // ...
    
    // Return updated attributes
    match fs::metadata(&real_path) {
        Ok(meta) => reply.attr(&TTL, &metadata_to_attr(&meta, ino)),
        Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
    }
}

fn truncate_via_path(path: &Path, size: u64) -> std::io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(size)
}
```

### lookup() - Register Inode Mapping

```rust
fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
    // Get parent path
    let parent_path = if parent == FUSE_ROOT_ID {
        PathBuf::new()
    } else {
        match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        }
    };
    
    // Build child path
    let rel_path = parent_path.join(name);
    let real_path = self.to_real(&rel_path);
    
    // Stat the file
    match fs::metadata(&real_path) {
        Ok(meta) => {
            let ino = meta.ino();
            
            // Register inode → path mapping
            self.register_inode(ino, rel_path);
            
            let attr = metadata_to_attr(&meta, ino);
            reply.entry(&TTL, &attr, 0);
        }
        Err(e) => reply.error(e.raw_os_error().unwrap_or(ENOENT)),
    }
}
```

### readdir() - Use Real Inodes

```rust
fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, 
           mut reply: ReplyDirectory) {
    let rel_path = if ino == FUSE_ROOT_ID {
        PathBuf::new()
    } else {
        match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        }
    };
    
    let real_path = self.to_real(&rel_path);
    
    let entries = match fs::read_dir(&real_path) {
        Ok(e) => e,
        Err(e) => { reply.error(e.raw_os_error().unwrap_or(ENOENT)); return; }
    };
    
    let mut all_entries = vec![
        (ino, FileType::Directory, "."),
        (ino, FileType::Directory, ".."),  // Simplified; should be parent
    ];
    
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            let child_ino = meta.ino();
            let file_type = if meta.is_dir() {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            
            let child_rel_path = rel_path.join(entry.file_name());
            self.register_inode(child_ino, child_rel_path);
            
            all_entries.push((child_ino, file_type, entry.file_name()));
        }
    }
    
    for (i, (ino, kind, name)) in all_entries.iter().enumerate().skip(offset as usize) {
        if reply.add(*ino, (i + 1) as i64, *kind, name) {
            break;
        }
    }
    
    reply.ok();
}
```

## Summary: What Changed from Original Design

| Aspect | Original Design | Simplified Design |
|--------|-----------------|-------------------|
| Inodes | Synthetic (counter) | **Real** (from `stat().ino()`) |
| File handles | Synthetic (counter) | **Real fd** (from `as_raw_fd()`) |
| Inode table | path ↔ inode bidirectional | **inode → path only** (reverse lookup) |
| File handle table | fh → FileHandle | **fd → FileHandle** (fd IS the fh) |
| Path mapping | PathForm enum, complex transforms | **Simple `source_dir.join(rel_path)`** |

## Benefits

1. **Less state to manage** - No synthetic counters to increment
2. **Real values** - Inodes and fds match what the kernel sees
3. **Hard links work** - Same inode for linked files
4. **Debugging easier** - `ls -i` shows real inodes, `lsof` shows real fds
5. **RAII safety** - File objects own fds, drop = close

## Filesystem Trait Methods

From `vendor/fuser/src/lib.rs`, implementation priority:

### Phase 1: Core (Must Have)
- [x] `lookup` - Path resolution, inode registration
- [x] `getattr` - Stat file
- [x] `readdir` - List directory
- [x] `open` - Store File, return fd as handle
- [x] `read` - pread() on fd
- [x] `write` - pwrite() on fd
- [x] `release` - Remove from table

### Phase 2: Modifications
- [x] `setattr` - chmod, truncate, utimes
- [ ] `create` - Create + open atomically
- [ ] `mkdir` - Create directory
- [ ] `unlink` - Remove file
- [ ] `rmdir` - Remove directory
- [ ] `rename` - Move/rename

### Phase 3: Advanced
- [ ] `mknod` - Create special files
- [ ] `symlink`, `readlink` - Symbolic links
- [ ] `link` - Hard links
- [ ] `flush`, `fsync` - Sync operations
- [ ] `statfs` - Filesystem stats
- [ ] `setxattr`, `getxattr`, `listxattr`, `removexattr` - Extended attributes
- [ ] `access` - Permission check
- [ ] `fallocate` - Preallocate space
- [ ] `lseek` - Seek (SEEK_HOLE/SEEK_DATA)
- [ ] `copy_file_range` - Efficient copy

## File Organization

```
crates/ize-lib/src/filesystems/
├── mod.rs
├── passthrough2.rs      # Single file implementation
├── error.rs             # Existing
├── passthrough.rs       # Original (keep for reference)
└── path_manager.rs      # Original (keep for reference)
```

Single file is fine since the design is now simple enough.

## Testing

### Update Existing Tests

The existing integration tests in `crates/ize-lib/tests/` currently test the original `PassthroughFS`. 
These need to be updated to test `PassthroughFS2`:

1. **Change imports**: `use ize_lib::filesystems::passthrough2::PassthroughFS2`
2. **Update constructor calls**: `PassthroughFS2::new(source_dir, mount_point)` (no db_path)
3. **Remove db-related test setup**: No database file creation needed

### Key Test Cases

Based on the issues documented in `fd_handling.md`, ensure these scenarios work:

```rust
#[test]
fn test_truncate_via_file_handle() {
    // The bug that originally failed:
    // 1. Create file through mount
    // 2. Open file (get fh)
    // 3. file.set_len(5) → calls setattr(fh, size=5)
    // 4. Should succeed because fh is valid and writable
}

#[test]
fn test_concurrent_reads_writes() {
    // Multiple threads reading/writing same file
    // pread/pwrite should handle this without seek races
}

#[test]
fn test_file_handle_lifecycle() {
    // open() → read() → write() → flush() → release()
    // fd should remain valid throughout
    // fd should be closed after release()
}
```

### Test File Location

```
crates/ize-lib/tests/
├── passthrough2_test.rs    # New tests for PassthroughFS2
├── integration_tests.rs    # Update to use PassthroughFS2
└── ...
```