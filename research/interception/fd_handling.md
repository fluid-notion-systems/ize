# File Descriptor Handling in PassthroughFS

## Executive Summary

The current `PassthroughFS` implementation has a **critical inconsistency** in how file descriptors (FDs) are handled. While `open()` and `create()` return duplicated file descriptors to FUSE, most operations **completely ignore** the passed file handle and re-open the file on every operation. This creates both correctness and performance issues.

## Current Implementation Analysis

### File Handle Creation

File handles are created in two places:

#### 1. `open()` (lines 1252-1315)

```rust
fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
    // ... path resolution ...
    
    match options.open(&real_path) {
        Ok(file) => {
            // Use file descriptor as file handle
            let fd = unsafe { libc::dup(file.as_raw_fd()) };
            reply.opened(fd as u64, ...);
        }
        // ...
    }
}
```

**Problem**: The `file` variable is dropped at the end of the match arm, closing the original FD. The duplicated FD (`fd`) is returned, but immediately after this function returns, the File object goes out of scope.

#### 2. `create()` (lines 1131-1250)

```rust
fn create(&mut self, ..., reply: ReplyCreate) {
    // ... file creation ...
    
    match options.open(&real_path) {
        Ok(file) => {
            // ... set permissions ...
            let fd = unsafe { libc::dup(file.as_raw_fd()) };
            reply.created(&TTL, &attr, 0, fd as u64, 0);
        }
        // ...
    }
}
```

**Same problem**: The `File` object is dropped, but the dup'd FD should remain valid.

### File Handle Usage (or Lack Thereof)

#### Operations That IGNORE the File Handle

| Operation | Lines | Behavior |
|-----------|-------|----------|
| `read()` | 873-926 | Opens file fresh with `File::open()`, ignores `fh` parameter |
| `write()` | 426-491 | Opens file fresh with `OpenOptions::new().write(true).open()`, ignores `fh` |

**Example from `write()`:**
```rust
fn write(&mut self, _req: &Request, ino: u64, fh: u64, offset: i64, data: &[u8], ...) {
    // fh is NEVER USED!
    
    // Instead, file is opened fresh every time:
    match fs::OpenOptions::new().write(true).open(&real_path) {
        Ok(mut file) => {
            file.seek(SeekFrom::Start(offset as u64))?;
            file.write(data)?;
        }
        // ...
    }
}
```

**Comment in code even acknowledges this:**
```rust
// Use Rust's File API instead of raw file descriptors
// This avoids issues with stale file handles after operations like truncate
```

#### Operations That USE the File Handle

| Operation | Lines | Behavior |
|-----------|-------|----------|
| `flush()` | 493-508 | Uses `fh` directly with `libc::fsync(fd)` |
| `release()` | 550-576 | Uses `fh` directly with `libc::close(fd)` |
| `setattr()` | 194-385 | Attempts to use `fh` with `libc::ftruncate()` - **THIS FAILS** |

**The `setattr()` failure:**
```rust
fn setattr(&mut self, ..., fh: Option<u64>, ...) {
    if let Some(fh) = fh {
        let fd = fh as i32;
        let result = unsafe { libc::ftruncate(fd, size as i64) };
        // Returns EINVAL (22) because fd is not valid for writing!
    }
}
```

### File Handle Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        FUSE File Handle Lifecycle                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  User: open("/mnt/file.txt")                                                │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ PassthroughFS::open()                                                │    │
│  │   1. Open real file → File object (owns fd=7)                       │    │
│  │   2. dup(7) → fd=8                                                   │    │
│  │   3. reply.opened(8)                                                 │    │
│  │   4. File object dropped → fd=7 CLOSED                              │    │
│  │   5. fd=8 remains open (orphaned, no Rust owner)                    │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│           │                                                                  │
│           │ FUSE stores fh=8                                                │
│           ▼                                                                  │
│  User: write(fd, "hello")                                                   │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ PassthroughFS::write(fh=8)                                          │    │
│  │   1. fh=8 is IGNORED                                                │    │
│  │   2. Opens file AGAIN → new File object                             │    │
│  │   3. Writes data                                                     │    │
│  │   4. File dropped → new fd closed                                   │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│           │                                                                  │
│           ▼                                                                  │
│  User: ftruncate(fd, 5)                                                     │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ PassthroughFS::setattr(fh=8, size=5)                                │    │
│  │   1. Tries libc::ftruncate(8, 5)                                    │    │
│  │   2. FAILS with EINVAL!                                             │    │
│  │      - fd=8 exists but was opened read-only or is in wrong state   │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│           │                                                                  │
│           ▼                                                                  │
│  User: close(fd)                                                            │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ PassthroughFS::release(fh=8)                                        │    │
│  │   1. libc::close(8)                                                 │    │
│  │   2. fd=8 finally closed                                            │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Root Cause of the `ftruncate` Failure

The `ftruncate` fails with EINVAL (error 22) because:

1. **The fd was created with wrong flags**: In `open()`, the file might be opened with different flags than what `ftruncate` needs. For example, if opened read-only.

2. **The fd is orphaned**: There's no `File` object in Rust that owns the fd. It's just a raw integer. This means:
   - No Rust safety guarantees
   - The fd could be closed by something else
   - The state of the fd is unknown

3. **No fd tracking**: PassthroughFS doesn't maintain any map of file handles to their state. It just returns a dup'd fd and hopes for the best.

## Why `read()` and `write()` Work

They work **despite** ignoring the file handle because they:
1. Open the file fresh each time
2. Use proper Rust `File` objects
3. Close cleanly via RAII

This is inefficient but correct.

## Recommended Solution: File Handle Table

### Data Structure

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

struct FileHandle {
    file: File,
    path: PathBuf,
    flags: i32,
}

struct PassthroughFS {
    // ... existing fields ...
    
    /// Map of FUSE file handles to actual File objects
    file_handles: Arc<RwLock<HashMap<u64, FileHandle>>>,
    
    /// Next file handle ID
    next_fh: AtomicU64,
}
```

### Implementation Changes

#### `open()` - Store the File object

```rust
fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
    // ... path resolution ...
    
    match options.open(&real_path) {
        Ok(file) => {
            // Generate a unique handle ID
            let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
            
            // Store the file object
            let handle = FileHandle {
                file,
                path: real_path.clone(),
                flags,
            };
            
            self.file_handles.write().unwrap().insert(fh, handle);
            
            reply.opened(fh, 0);
        }
        // ...
    }
}
```

#### `write()` - Use the stored File

```rust
fn write(&mut self, _req: &Request, ino: u64, fh: u64, offset: i64, data: &[u8], ...) {
    let handles = self.file_handles.read().unwrap();
    
    match handles.get(&fh) {
        Some(handle) => {
            // Use the stored file directly
            // Note: Need interior mutability for seek/write
            handle.file.seek(SeekFrom::Start(offset as u64))?;
            handle.file.write(data)?;
        }
        None => {
            error!("write: invalid file handle {}", fh);
            reply.error(libc::EBADF);
        }
    }
}
```

#### `setattr()` - Use the stored File for truncate

```rust
fn setattr(&mut self, ..., fh: Option<u64>, ...) {
    if let Some(size) = size {
        if let Some(fh) = fh {
            let handles = self.file_handles.read().unwrap();
            if let Some(handle) = handles.get(&fh) {
                // Use the actual File object
                handle.file.set_len(size)?;
            }
        } else {
            // Fallback: open file fresh
            let file = OpenOptions::new().write(true).open(&real_path)?;
            file.set_len(size)?;
        }
    }
}
```

#### `release()` - Remove from table

```rust
fn release(&mut self, _req: &Request, ino: u64, fh: u64, ...) {
    // Remove from table - File will be dropped and closed automatically
    self.file_handles.write().unwrap().remove(&fh);
    reply.ok();
}
```

## Alternative: Keep Current Approach

If maintaining a file handle table is too complex, the current approach can be made correct by:

1. **Always ignore the fh parameter** - Open files fresh for every operation
2. **Document this behavior** - Make it clear this is intentional
3. **Remove ftruncate code path** - Always use the fallback in `setattr()`

This is less efficient but simpler and correct.

## Performance Implications

### Current Implementation
- Every `read()` and `write()` opens the file fresh
- System call overhead: `open()` + operation + `close()` for each operation
- No benefit from kernel page cache hints tied to file descriptors

### With File Handle Table
- File opened once, used many times
- Single system call per operation
- Better cache behavior
- More complex code and synchronization

## Comparison with Other FUSE Filesystems

### libfuse examples (passthrough.c)
- Uses a file handle table (`lo_inode` struct with `fd` field)
- Stores actual file descriptors
- Properly tracks open/close lifecycle

### sshfs
- Maintains connection state per file handle
- Maps FUSE handles to SFTP handles

### bindfs
- Uses `dup()` like us, but with proper fd tracking
- Maintains refcounts on file descriptors

## Recommendations

### Short Term (Fix the Bug)
1. In `setattr()`, always fall back to opening the file if `ftruncate()` fails
2. This is what the stashed fix does

### Medium Term (Clean Up)
1. Remove all pretense of using file handles in `read()`/`write()`
2. Document that file handles are only used for `flush()` and `release()`
3. Update `setattr()` to never use the fh for truncate

### Long Term (Proper Implementation)
1. Implement a file handle table as described above
2. Store `File` objects properly
3. Use the stored handles for all operations
4. This enables proper `O_APPEND` semantics, locking, etc.

## Test Case

The failing test demonstrates the issue:

```rust
#[test]
fn test_file_truncate_marks_as_dirty() {
    // 1. Mount FUSE filesystem
    // 2. Create file through mount
    // 3. Open file (FUSE returns fh=8)
    // 4. Call file.set_len(5) 
    //    → FUSE calls setattr(fh=8, size=5)
    //    → PassthroughFS tries ftruncate(8, 5)
    //    → FAILS with EINVAL because fd 8 is not writable
}
```

## Conclusion

The current implementation is **functionally broken** for operations that try to use the file handle (`setattr` with truncate, potentially `flush`). The workaround is to always re-open files, which is what `read()` and `write()` already do. A proper fix requires implementing a file handle table to track open files and their states.