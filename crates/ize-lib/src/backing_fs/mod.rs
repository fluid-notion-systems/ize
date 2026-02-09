//! Backing filesystem abstraction.
//!
//! The [`BackingFs`] trait provides directory-relative filesystem operations so
//! the FUSE passthrough layer can work against any backing implementation
//! without depending on a specific one.
//!
//! All paths passed to trait methods are **relative** to the backing store root.
//! An empty path (or `Path::new("")`) refers to the root itself.
//!
//! [`RawFd`] values returned by [`BackingFs::open_file`] are owned by the
//! caller, who is responsible for closing them via [`BackingFs::close_fd`].

pub mod libc_impl;

use std::ffi::OsString;
use std::io;
use std::os::unix::io::RawFd;
use std::path::Path;

// Re-exports
pub use libc_impl::LibcBackingFs;

// ---------------------------------------------------------------------------
// DirEntry
// ---------------------------------------------------------------------------

/// A single directory entry returned by [`BackingFs::readdir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Inode number.
    pub ino: u64,
    /// File type as returned by `readdir(3)` (`DT_REG`, `DT_DIR`, …).
    pub dtype: u8,
    /// Entry name (file/directory name, **not** a full path).
    pub name: OsString,
}

// ---------------------------------------------------------------------------
// BackingFs trait
// ---------------------------------------------------------------------------

/// Abstraction over directory-relative filesystem operations.
///
/// Implementations must be safe to call from multiple threads concurrently
/// (`Send + Sync`) because FUSE dispatches requests in parallel.
///
/// Every method returns [`io::Result`] so that raw `errno` values propagate
/// naturally through the standard library's error type.
pub trait BackingFs: Send + Sync {
    // -- Stat ---------------------------------------------------------------

    /// Stat a path relative to the backing store root.
    ///
    /// Behaves like `fstatat(base, rel, AT_SYMLINK_NOFOLLOW)`.
    fn stat(&self, rel: &Path) -> io::Result<libc::stat>;

    /// Stat an already-open file descriptor.
    ///
    /// Behaves like `fstat(fd)`.
    fn fstat(&self, fd: RawFd) -> io::Result<libc::stat>;

    // -- File I/O -----------------------------------------------------------

    /// Open (or create) a file relative to the backing store root.
    ///
    /// `flags` and `mode` mirror the `openat(2)` parameters.  The returned
    /// [`RawFd`] is owned by the caller who **must** eventually call
    /// [`close_fd`](BackingFs::close_fd).
    fn open_file(&self, rel: &Path, flags: i32, mode: u32) -> io::Result<RawFd>;

    /// Read from a file descriptor at the given offset without changing the
    /// file's seek position.
    fn pread(&self, fd: RawFd, buf: &mut [u8], offset: i64) -> io::Result<usize>;

    /// Write to a file descriptor at the given offset without changing the
    /// file's seek position.
    fn pwrite(&self, fd: RawFd, data: &[u8], offset: i64) -> io::Result<usize>;

    /// Flush in-core data for `fd` to stable storage.
    fn fsync(&self, fd: RawFd) -> io::Result<()>;

    /// Close a file descriptor previously obtained from [`open_file`](BackingFs::open_file).
    fn close_fd(&self, fd: RawFd);

    /// Truncate (or extend) a file to `size` bytes.
    fn ftruncate(&self, fd: RawFd, size: u64) -> io::Result<()>;

    // -- Directory ops ------------------------------------------------------

    /// Create a directory relative to the backing store root.
    fn mkdir(&self, rel: &Path, mode: u32) -> io::Result<()>;

    /// Remove an empty directory relative to the backing store root.
    fn rmdir(&self, rel: &Path) -> io::Result<()>;

    /// List entries in a directory relative to the backing store root.
    ///
    /// The returned vector includes `.` and `..` only if the underlying
    /// implementation provides them.
    fn readdir(&self, rel: &Path) -> io::Result<Vec<DirEntry>>;

    // -- Path ops -----------------------------------------------------------

    /// Remove a non-directory entry relative to the backing store root.
    fn unlink(&self, rel: &Path) -> io::Result<()>;

    /// Rename an entry.  Both `old` and `new` are relative to the backing
    /// store root.
    fn rename(&self, old: &Path, new: &Path) -> io::Result<()>;

    // -- Metadata -----------------------------------------------------------

    /// Change file mode bits.
    fn chmod(&self, rel: &Path, mode: u32) -> io::Result<()>;

    /// Change file owner and/or group.
    ///
    /// A `None` value for `uid` or `gid` means "don't change".
    fn chown(&self, rel: &Path, uid: Option<u32>, gid: Option<u32>) -> io::Result<()>;

    /// Set access and modification times.
    fn utimens(&self, rel: &Path, atime: &libc::timespec, mtime: &libc::timespec)
        -> io::Result<()>;

    /// Check accessibility of a file relative to the backing store root.
    ///
    /// `mask` uses the same constants as `access(2)` / `faccessat(2)`.
    fn access(&self, rel: &Path, mask: i32) -> io::Result<()>;

    // -- Filesystem ---------------------------------------------------------

    /// Return filesystem statistics for the backing store.
    fn statvfs(&self) -> io::Result<libc::statvfs>;
}
