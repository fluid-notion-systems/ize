# PassthroughFS2 Idiomatic Rust Refactoring Plan

## Overview

This document analyzes the current `passthrough2.rs` implementation, identifying all uses of `unsafe`, raw file descriptors, and `libc` calls. The goal is to replace these with safe, idiomatic Rust alternatives where possible.

## Current State Analysis

### Summary of Issues

| Category | Count | Severity |
|----------|-------|----------|
| `unsafe` blocks | 10 | High |
| `libc::` calls | 18+ | Medium |
| Raw fd usage (`as_raw_fd`, `fh as i32`) | 8 | Medium |
| `CString` manual construction | 4 | Low |

### Detailed Inventory of `unsafe` and `libc` Usage

#### 1. `read()` - Lines 696-710
```rust
let n = unsafe {
    libc::pread(
        fd,
        buf.as_mut_ptr() as *mut libc::c_void,
        size as usize,
        offset,
    )
};
```
**Problem**: Uses raw `pread` syscall instead of Rust's `File` API.

**Solution**: Use `std::os::unix::fs::FileExt::read_at()` on the stored `File`:
```rust
use std::os::unix::fs::FileExt;

// In read():
let handles = self.file_handles.read().unwrap();
if let Some(handle) = handles.get(&fd) {
    let mut buf = vec![0u8; size as usize];
    match handle.file.read_at(&mut buf, offset as u64) {
        Ok(n) => {
            buf.truncate(n);
            reply.data(&buf);
        }
        Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
    }
} else {
    reply.error(EBADF);
}
```

#### 2. `write()` - Lines 752-753
```rust
let n = unsafe { 
    libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset) 
};
```
**Problem**: Uses raw `pwrite` syscall.

**Solution**: Use `std::os::unix::fs::FileExt::write_at()`:
```rust
use std::os::unix::fs::FileExt;

// In write():
let handles = self.file_handles.read().unwrap();
if let Some(handle) = handles.get(&fd) {
    match handle.file.write_at(data, offset as u64) {
        Ok(n) => reply.written(n as u32),
        Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
    }
} else {
    reply.error(EBADF);
}
```

#### 3. `flush()` - Lines 779-780
```rust
if unsafe { libc::fsync(fd) } == 0 {
```
**Problem**: Uses raw `fsync` syscall.

**Solution**: Use `File::sync_all()`:
```rust
let handles = self.file_handles.read().unwrap();
if let Some(handle) = handles.get(&fd) {
    match handle.file.sync_all() {
        Ok(()) => reply.ok(),
        Err(e) => {
            if e.raw_os_error() == Some(EBADF) {
                reply.ok();
            } else {
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }
} else {
    reply.ok(); // File not found, already closed
}
```

#### 4. `fsync()` - Lines 818-822
```rust
let ret = if datasync {
    unsafe { libc::fdatasync(fd) }
} else {
    unsafe { libc::fsync(fd) }
};
```
**Problem**: Uses raw `fsync`/`fdatasync` syscalls.

**Solution**: Use `File::sync_all()` or `File::sync_data()`:
```rust
let handles = self.file_handles.read().unwrap();
if let Some(handle) = handles.get(&fd) {
    let result = if datasync {
        handle.file.sync_data()
    } else {
        handle.file.sync_all()
    };
    match result {
        Ok(()) => reply.ok(),
        Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
    }
} else {
    reply.error(EBADF);
}
```

#### 5. `setattr()` - Truncate via ftruncate - Lines 388-394
```rust
let ret = unsafe { libc::ftruncate(fd, new_size as i64) };
```
**Problem**: Uses raw `ftruncate` syscall.

**Solution**: Use `File::set_len()`:
```rust
if let Some(handle) = handles.get(&fd) {
    if (handle.flags & (libc::O_WRONLY | libc::O_RDWR)) != 0 {
        match handle.file.set_len(new_size) {
            Ok(()) => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        // Fall back to path-based
        drop(handles);
        Self::truncate_via_path(&real_path, new_size)
    }
}
```

#### 6. `setattr()` - chown - Lines 431-450
```rust
let ret = unsafe {
    libc::chown(
        path_cstr.as_ptr(),
        if uid.is_some() { new_uid } else { u32::MAX as libc::uid_t },
        if gid.is_some() { new_gid } else { u32::MAX as libc::gid_t },
    )
};
```
**Problem**: Uses raw `chown` syscall with manual CString handling.

**Solution**: Use `nix::unistd::chown()`:
```rust
use nix::unistd::{chown, Uid, Gid};

let uid_opt = uid.map(|u| Uid::from_raw(u));
let gid_opt = gid.map(|g| Gid::from_raw(g));

match chown(&real_path, uid_opt, gid_opt) {
    Ok(()) => { /* success */ }
    Err(e) => {
        reply.error(e as i32);
        return;
    }
}
```

#### 7. `setattr()` - utimensat - Lines 478-479
```rust
let ret = unsafe { 
    libc::utimensat(libc::AT_FDCWD, path_cstr.as_ptr(), times.as_ptr(), 0) 
};
```
**Problem**: Uses raw `utimensat` syscall with manual timespec construction.

**Solution**: Use `nix::sys::stat::utimensat()` or `filetime` crate:
```rust
use nix::sys::stat::{utimensat, UtimensatFlags};
use nix::sys::time::TimeSpec;

let atime_ts = match atime {
    Some(TimeOrNow::SpecificTime(st)) => {
        let d = st.duration_since(UNIX_EPOCH).unwrap_or_default();
        TimeSpec::new(d.as_secs() as i64, d.subsec_nanos() as i64)
    }
    Some(TimeOrNow::Now) => TimeSpec::UTIME_NOW,
    None => TimeSpec::UTIME_OMIT,
};
// Similar for mtime_ts...

match utimensat(None, &real_path, &atime_ts, &mtime_ts, UtimensatFlags::NoFollowSymlink) {
    Ok(()) => { /* success */ }
    Err(e) => {
        reply.error(e as i32);
        return;
    }
}
```

#### 8. `access()` - Lines 1218-1219
```rust
let ret = unsafe { libc::access(path_cstr.as_ptr(), mask) };
```
**Problem**: Uses raw `access` syscall.

**Solution**: Use `nix::unistd::access()`:
```rust
use nix::unistd::{access, AccessFlags};

let flags = AccessFlags::from_bits_truncate(mask as i32);
match access(&real_path, flags) {
    Ok(()) => reply.ok(),
    Err(e) => reply.error(e as i32),
}
```

#### 9. `statfs()` - Lines 1243-1244
```rust
let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
let ret = unsafe { libc::statvfs(path_cstr.as_ptr(), &mut stat) };
```
**Problem**: Uses raw `statvfs` syscall with `mem::zeroed()`.

**Solution**: Use `nix::sys::statvfs::statvfs()`:
```rust
use nix::sys::statvfs::statvfs;

match statvfs(&self.source_dir) {
    Ok(stat) => {
        reply.statfs(
            stat.blocks(),
            stat.blocks_free(),
            stat.blocks_available(),
            stat.files(),
            stat.files_free(),
            stat.block_size() as u32,
            stat.name_max() as u32,
            stat.fragment_size() as u32,
        );
    }
    Err(e) => {
        reply.error(e as i32);
    }
}
```

### Other Issues

#### 10. Flag Constants from `libc`
Currently uses: `libc::O_RDONLY`, `libc::O_WRONLY`, `libc::O_RDWR`, `libc::O_APPEND`, `libc::O_TRUNC`, `libc::EROFS`, etc.

**Analysis**: These are just constants, not unsafe. However, they could be replaced with `nix::fcntl::OFlag` for type safety:
```rust
use nix::fcntl::OFlag;

// Instead of: flags & libc::O_ACCMODE
// Use: OFlag::from_bits_truncate(flags).contains(OFlag::O_RDWR)
```

**Recommendation**: Keep `libc` constants for now - they're safe and changing them adds complexity without much benefit.

#### 11. File Handle Architecture

**Current Design**:
- Store `File` objects in a `HashMap<i32, FileHandle>` keyed by raw fd
- Use the raw fd as the FUSE file handle (`fh`)
- Look up `File` by converting `fh` back to `i32`

**Problem**: This is indirect - we already have the `File`, but we extract its fd then look it up again.

**Better Design**: Use a simple incrementing counter as the file handle key:
```rust
struct PassthroughFS2 {
    // ...
    next_fh: AtomicU64,
    file_handles: RwLock<HashMap<u64, FileHandle>>,  // Key is our generated fh, not fd
}

// In open():
let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
self.file_handles.write().unwrap().insert(fh, FileHandle { file, ... });
reply.opened(fh, 0);

// In read():
let handles = self.file_handles.read().unwrap();
if let Some(handle) = handles.get(&fh) {
    // Use handle.file directly - no fd conversion needed
}
```

This eliminates all `as_raw_fd()` calls and `fh as i32` conversions.

## Implementation Plan

### Phase 1: Replace read/write/sync with FileExt (High Impact)
1. Change `file_handles` key from `i32` (fd) to `u64` (generated fh)
2. Add `next_fh: AtomicU64` counter
3. Update `open()` to use generated fh
4. Update `read()` to use `FileExt::read_at()`
5. Update `write()` to use `FileExt::write_at()`
6. Update `flush()` to use `File::sync_all()`
7. Update `fsync()` to use `File::sync_all()`/`sync_data()`
8. Update `setattr()` truncate to use `File::set_len()`

**Files Changed**: `passthrough2.rs`
**Estimated Effort**: 1-2 hours
**Risk**: Low - straightforward API changes

### Phase 2: Replace chown/utimensat/access/statfs with nix (Medium Impact)
1. Update `setattr()` chown to use `nix::unistd::chown()`
2. Update `setattr()` utimensat to use `nix::sys::stat::utimensat()`
3. Update `access()` to use `nix::unistd::access()`
4. Update `statfs()` to use `nix::sys::statvfs::statvfs()`

**Files Changed**: `passthrough2.rs`
**Estimated Effort**: 1-2 hours
**Risk**: Low - nix provides safe wrappers

### Phase 3: Clean up remaining libc usage (Low Impact)
1. Review remaining `libc::` usages (error codes, flags)
2. Determine if any can be replaced with Rust equivalents
3. Document any that must remain

**Files Changed**: `passthrough2.rs`
**Estimated Effort**: 30 minutes
**Risk**: Very Low

## Expected Outcome

After refactoring:

| Metric | Before | After |
|--------|--------|-------|
| `unsafe` blocks | 10 | 0 |
| `libc::` function calls | 10 | 0 |
| `libc::` constants | ~15 | ~15 (acceptable) |
| Raw fd conversions | 8 | 0 |

## Benefits

1. **Safety**: No more `unsafe` blocks means the compiler can verify memory safety
2. **Readability**: Rust's `File` API is more expressive than raw syscalls
3. **Maintainability**: Less low-level code to reason about
4. **Error Handling**: Rust's `Result` types integrate better than checking return codes
5. **Cross-platform potential**: While FUSE is Linux-specific, the code patterns are more portable

## Testing Strategy

1. All existing tests should pass unchanged
2. Add specific tests for edge cases:
   - Read/write at various offsets
   - Concurrent reads/writes to same file
   - Truncate via file handle vs path
   - Permission changes
   - Timestamp updates
3. Manual testing with mounted filesystem

## Dependencies

Already available in `Cargo.toml`:
- `nix = { version = "0.30.1", features = ["fs"] }`

May need to add features:
- `nix = { version = "0.30.1", features = ["fs", "user"] }` for `chown`

## Open Questions

1. **Performance**: Will using `FileExt` traits be slower than raw syscalls?
   - Likely negligible - they're thin wrappers
   - Benchmark if concerned

2. **Error mapping**: Does `nix` return errors compatible with FUSE's expectations?
   - Yes, `nix::Error` can be converted to errno values

3. **Should we keep the fd-as-fh design?**
   - No, the generated-fh design is cleaner and more idiomatic