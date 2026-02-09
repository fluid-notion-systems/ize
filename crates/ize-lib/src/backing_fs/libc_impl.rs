//! libc-based [`BackingFs`](super::BackingFs) implementation.
//!
//! [`LibcBackingFs`] implements the `BackingFs` trait using raw `*at()` libc
//! syscalls against a pre-opened directory file descriptor.  This fd is
//! obtained **before** the FUSE mount is established so that all operations
//! bypass the FUSE layer entirely.
//!
//! The full implementation is tracked by issue **ize-jvi.2**.

use std::os::unix::io::RawFd;

/// A [`BackingFs`](super::BackingFs) backed by a pre-opened directory fd and
/// raw libc `*at()` syscalls.
///
/// # Lifecycle
///
/// * The caller opens `base_fd` with `O_RDONLY | O_DIRECTORY` **before**
///   mounting the FUSE filesystem.
/// * `LibcBackingFs` does **not** close `base_fd` on drop — the caller owns
///   its lifetime.
/// * Individual file descriptors returned by
///   [`open_file`](super::BackingFs::open_file) **are** the caller's
///   responsibility to close via [`close_fd`](super::BackingFs::close_fd).
pub struct LibcBackingFs {
    /// Pre-opened directory fd that anchors all `*at()` operations.
    base_fd: RawFd,
}

impl LibcBackingFs {
    /// Wrap an existing directory file descriptor.
    ///
    /// # Safety contract (not `unsafe` but important)
    ///
    /// `base_fd` **must** be a valid, open file descriptor referring to a
    /// directory.  It must remain open for the entire lifetime of this struct.
    pub fn new(base_fd: RawFd) -> Self {
        Self { base_fd }
    }

    /// Return the underlying base directory fd.
    pub fn base_fd(&self) -> RawFd {
        self.base_fd
    }
}
