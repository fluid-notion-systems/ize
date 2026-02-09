//! libc-based [`BackingFs`](super::BackingFs) implementation.
//!
//! [`LibcBackingFs`] implements the `BackingFs` trait using raw `*at()` libc
//! syscalls against a pre-opened directory file descriptor.  This fd is
//! obtained **before** the FUSE mount is established so that all operations
//! bypass the FUSE layer entirely — no recursive re-entry.
//!
//! # Construction
//!
//! Use [`LibcBackingFs::open_dir`] to open a directory by path — the fd is
//! owned and automatically closed on drop.  Use [`LibcBackingFs::from_raw_fd`]
//! when the caller already holds an fd and retains ownership.

use std::ffi::{CStr, CString, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::Path;

use super::{BackingFs, DirEntry};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a relative `Path` to a `CString` suitable for `*at()` syscalls.
///
/// An empty path (representing the backing-store root) is mapped to `"."`.
fn rel_cpath(rel: &Path) -> io::Result<CString> {
    let bytes = rel.as_os_str().as_bytes();
    if bytes.is_empty() {
        CString::new(".").map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    } else {
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interior nul in path"))
    }
}

/// Open a directory fd relative to `base_fd` for use with `fdopendir`.
///
/// The returned fd must be closed by the caller (or handed to `fdopendir`
/// which will close it via `closedir`).
fn open_dir_for(base_fd: RawFd, rel: &Path) -> io::Result<RawFd> {
    let c_path = rel_cpath(rel)?;
    let fd = unsafe {
        libc::openat(
            base_fd,
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

// ---------------------------------------------------------------------------
// LibcBackingFs
// ---------------------------------------------------------------------------

/// A [`BackingFs`] backed by a pre-opened directory fd and raw libc `*at()`
/// syscalls.
///
/// # Construction
///
/// * [`open_dir`](Self::open_dir) — open a directory by path.  The fd is
///   **owned** and automatically closed when the struct is dropped.
/// * [`from_raw_fd`](Self::from_raw_fd) — wrap an existing fd.  The caller
///   retains ownership; the fd is **not** closed on drop.
///
/// # Lifecycle
///
/// * The `base_fd` must be opened **before** mounting the FUSE filesystem
///   so that `*at()` calls resolve against the underlying inode.
/// * Individual file descriptors returned by
///   [`open_file`](BackingFs::open_file) **are** the caller's responsibility
///   to close via [`close_fd`](BackingFs::close_fd).
#[derive(Debug)]
pub struct LibcBackingFs {
    /// Pre-opened directory fd that anchors all `*at()` operations.
    base_fd: RawFd,
    /// When `true`, `base_fd` is closed on [`Drop`].
    owned: bool,
}

// SAFETY: The raw fd is just an integer handle.  All operations go through
// libc syscalls which are inherently thread-safe at the kernel level.
unsafe impl Send for LibcBackingFs {}
unsafe impl Sync for LibcBackingFs {}

impl Drop for LibcBackingFs {
    fn drop(&mut self) {
        if self.owned {
            unsafe { libc::close(self.base_fd) };
        }
    }
}

impl LibcBackingFs {
    /// Open a directory by path and return a `LibcBackingFs` that **owns**
    /// the file descriptor.
    ///
    /// The fd is opened with `O_RDONLY | O_DIRECTORY | O_CLOEXEC` and will
    /// be automatically closed when this struct is dropped.
    ///
    /// This should be called **before** mounting the FUSE filesystem so that
    /// the kernel resolves the fd to the underlying inode.
    pub fn open_dir(path: &Path) -> io::Result<Self> {
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path contains interior nul")
        })?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self {
                base_fd: fd,
                owned: true,
            })
        }
    }

    /// Wrap an existing directory file descriptor **without** taking
    /// ownership.
    ///
    /// The fd will **not** be closed when this struct is dropped — the
    /// caller is responsible for its lifetime.
    ///
    /// # Safety contract (not `unsafe` but important)
    ///
    /// `base_fd` **must** be a valid, open file descriptor referring to a
    /// directory.  It must remain open for the entire lifetime of this struct.
    pub fn from_raw_fd(base_fd: RawFd) -> Self {
        Self {
            base_fd,
            owned: false,
        }
    }

    /// Return the underlying base directory fd.
    pub fn base_fd(&self) -> RawFd {
        self.base_fd
    }
}

impl BackingFs for LibcBackingFs {
    // -- Stat ---------------------------------------------------------------

    fn stat(&self, rel: &Path) -> io::Result<libc::stat> {
        let c_path = rel_cpath(rel)?;
        unsafe {
            let mut stat_buf: libc::stat = std::mem::zeroed();
            let rc = libc::fstatat(
                self.base_fd,
                c_path.as_ptr(),
                &mut stat_buf,
                libc::AT_SYMLINK_NOFOLLOW,
            );
            if rc < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(stat_buf)
            }
        }
    }

    fn fstat(&self, fd: RawFd) -> io::Result<libc::stat> {
        unsafe {
            let mut stat_buf: libc::stat = std::mem::zeroed();
            let rc = libc::fstat(fd, &mut stat_buf);
            if rc < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(stat_buf)
            }
        }
    }

    // -- File I/O -----------------------------------------------------------

    fn open_file(&self, rel: &Path, flags: i32, mode: u32) -> io::Result<RawFd> {
        let c_path = rel_cpath(rel)?;
        let fd = unsafe {
            libc::openat(
                self.base_fd,
                c_path.as_ptr(),
                flags | libc::O_CLOEXEC,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn pread(&self, fd: RawFd, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        let n =
            unsafe { libc::pread(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), offset) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn pwrite(&self, fd: RawFd, data: &[u8], offset: i64) -> io::Result<usize> {
        let n =
            unsafe { libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn fsync(&self, fd: RawFd) -> io::Result<()> {
        let rc = unsafe { libc::fsync(fd) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn close_fd(&self, fd: RawFd) {
        unsafe {
            libc::close(fd);
        }
    }

    fn ftruncate(&self, fd: RawFd, size: u64) -> io::Result<()> {
        let rc = unsafe { libc::ftruncate(fd, size as libc::off_t) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    // -- Directory ops ------------------------------------------------------

    fn mkdir(&self, rel: &Path, mode: u32) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let rc = unsafe { libc::mkdirat(self.base_fd, c_path.as_ptr(), mode as libc::mode_t) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn rmdir(&self, rel: &Path) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let rc = unsafe { libc::unlinkat(self.base_fd, c_path.as_ptr(), libc::AT_REMOVEDIR) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn readdir(&self, rel: &Path) -> io::Result<Vec<DirEntry>> {
        // Open the target directory relative to base_fd.
        let dir_fd = open_dir_for(self.base_fd, rel)?;

        // dup() so that fdopendir (which takes ownership and closes the fd via
        // closedir) doesn't steal our dir_fd.
        let dup_fd = unsafe { libc::dup(dir_fd) };
        // We no longer need the original fd — close it now.
        unsafe { libc::close(dir_fd) };

        if dup_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let dirp = unsafe { libc::fdopendir(dup_fd) };
        if dirp.is_null() {
            unsafe { libc::close(dup_fd) };
            return Err(io::Error::last_os_error());
        }

        let mut entries = Vec::new();
        loop {
            // Reset errno so we can distinguish end-of-directory from error.
            unsafe { *libc::__errno_location() = 0 };
            let ent = unsafe { libc::readdir(dirp) };
            if ent.is_null() {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(0) {
                    break; // normal end of directory
                }
                unsafe { libc::closedir(dirp) };
                return Err(e);
            }
            let ent = unsafe { &*ent };
            let name_cstr = unsafe { CStr::from_ptr(ent.d_name.as_ptr()) };
            let name = OsString::from_vec(name_cstr.to_bytes().to_vec());
            entries.push(DirEntry {
                ino: ent.d_ino,
                dtype: ent.d_type,
                name,
            });
        }
        unsafe { libc::closedir(dirp) }; // also closes dup_fd
        Ok(entries)
    }

    // -- Path ops -----------------------------------------------------------

    fn unlink(&self, rel: &Path) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let rc = unsafe { libc::unlinkat(self.base_fd, c_path.as_ptr(), 0) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn rename(&self, old: &Path, new: &Path) -> io::Result<()> {
        let c_old = rel_cpath(old)?;
        let c_new = rel_cpath(new)?;
        let rc =
            unsafe { libc::renameat(self.base_fd, c_old.as_ptr(), self.base_fd, c_new.as_ptr()) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    // -- Metadata -----------------------------------------------------------

    fn chmod(&self, rel: &Path, mode: u32) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let rc = unsafe { libc::fchmodat(self.base_fd, c_path.as_ptr(), mode as libc::mode_t, 0) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn chown(&self, rel: &Path, uid: Option<u32>, gid: Option<u32>) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        // -1 means "don't change" for chown/fchownat.
        let raw_uid = uid.map_or(-1i32 as libc::uid_t, |u| u as libc::uid_t);
        let raw_gid = gid.map_or(-1i32 as libc::gid_t, |g| g as libc::gid_t);
        let rc = unsafe {
            libc::fchownat(
                self.base_fd,
                c_path.as_ptr(),
                raw_uid,
                raw_gid,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn utimens(
        &self,
        rel: &Path,
        atime: &libc::timespec,
        mtime: &libc::timespec,
    ) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let times = [*atime, *mtime];
        let rc = unsafe {
            libc::utimensat(
                self.base_fd,
                c_path.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn access(&self, rel: &Path, mask: i32) -> io::Result<()> {
        let c_path = rel_cpath(rel)?;
        let rc = unsafe {
            libc::faccessat(
                self.base_fd,
                c_path.as_ptr(),
                mask,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    // -- Filesystem ---------------------------------------------------------

    fn statvfs(&self) -> io::Result<libc::statvfs> {
        unsafe {
            let mut buf: libc::statvfs = std::mem::zeroed();
            let rc = libc::fstatvfs(self.base_fd, &mut buf);
            if rc < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(buf)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    /// Helper: create a temp dir and return a `LibcBackingFs` rooted at it.
    /// The `File` keeps the fd alive for the lifetime of the test.
    fn make_backing(tmpdir: &std::path::Path) -> (fs::File, LibcBackingFs) {
        let dir_file = fs::File::open(tmpdir).expect("open tmpdir");
        let backing = LibcBackingFs::from_raw_fd(dir_file.as_raw_fd());
        (dir_file, backing)
    }

    #[test]
    fn open_dir_and_stat() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("probe.txt"), "hi").unwrap();

        let backing = LibcBackingFs::open_dir(tmp.path()).expect("open_dir");
        let st = backing.stat(Path::new("probe.txt")).unwrap();
        assert_eq!(st.st_size, 2);
        // fd is closed automatically when `backing` drops
    }

    #[test]
    fn open_dir_nonexistent_fails() {
        let err = LibcBackingFs::open_dir(Path::new("/nonexistent/path/unlikely")).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
    }

    #[test]
    fn from_raw_fd_does_not_close() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_file = fs::File::open(tmp.path()).unwrap();
        let fd = dir_file.as_raw_fd();

        {
            let backing = LibcBackingFs::from_raw_fd(fd);
            // backing goes out of scope — should NOT close the fd
            let _ = backing.stat(Path::new(""));
        }

        // fd should still be valid because dir_file owns it
        // and from_raw_fd did not close it
        let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::fstat(fd, &mut stat_buf) };
        assert_eq!(rc, 0, "fd should still be valid after from_raw_fd drop");
    }

    #[test]
    fn stat_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let st = backing.stat(Path::new("")).expect("stat root");
        // Root should be a directory.
        assert_eq!(st.st_mode & libc::S_IFMT, libc::S_IFDIR);
    }

    #[test]
    fn mkdir_and_readdir() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        backing.mkdir(Path::new("subdir"), 0o755).expect("mkdir");

        let entries = backing.readdir(Path::new("")).expect("readdir root");
        let names: Vec<_> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
        assert!(names.contains(&"subdir"), "entries: {:?}", names);
    }

    #[test]
    fn open_write_read_close() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let fd = backing
            .open_file(
                Path::new("hello.txt"),
                libc::O_CREAT | libc::O_WRONLY,
                0o644,
            )
            .expect("open_file create");

        let msg = b"hello backing_fs";
        let written = backing.pwrite(fd, msg, 0).expect("pwrite");
        assert_eq!(written, msg.len());
        backing.close_fd(fd);

        // Re-open read-only and verify.
        let fd2 = backing
            .open_file(Path::new("hello.txt"), libc::O_RDONLY, 0)
            .expect("open_file read");
        let mut buf = vec![0u8; 64];
        let n = backing.pread(fd2, &mut buf, 0).expect("pread");
        assert_eq!(&buf[..n], msg);
        backing.close_fd(fd2);
    }

    #[test]
    fn fstat_open_fd() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("probe.txt");
        fs::write(&file_path, "abc").unwrap();

        let (_hold, backing) = make_backing(tmp.path());

        let fd = backing
            .open_file(Path::new("probe.txt"), libc::O_RDONLY, 0)
            .expect("open");
        let st = backing.fstat(fd).expect("fstat");
        assert_eq!(st.st_size, 3);
        backing.close_fd(fd);
    }

    #[test]
    fn ftruncate_extends_and_shrinks() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let fd = backing
            .open_file(Path::new("trunc.txt"), libc::O_CREAT | libc::O_RDWR, 0o644)
            .expect("open");

        backing.pwrite(fd, b"hello world", 0).unwrap();

        // Shrink
        backing.ftruncate(fd, 5).expect("ftruncate shrink");
        let st = backing.fstat(fd).unwrap();
        assert_eq!(st.st_size, 5);

        // Extend
        backing.ftruncate(fd, 100).expect("ftruncate extend");
        let st = backing.fstat(fd).unwrap();
        assert_eq!(st.st_size, 100);

        backing.close_fd(fd);
    }

    #[test]
    fn unlink_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("bye.txt"), "gone").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        backing.unlink(Path::new("bye.txt")).expect("unlink");

        assert!(backing.stat(Path::new("bye.txt")).is_err());
    }

    #[test]
    fn rmdir_empty() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("empty_dir")).unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        backing.rmdir(Path::new("empty_dir")).expect("rmdir");

        assert!(backing.stat(Path::new("empty_dir")).is_err());
    }

    #[test]
    fn rename_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("old.txt"), "data").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        backing
            .rename(Path::new("old.txt"), Path::new("new.txt"))
            .expect("rename");

        assert!(backing.stat(Path::new("old.txt")).is_err());
        assert!(backing.stat(Path::new("new.txt")).is_ok());
    }

    #[test]
    fn chmod_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("mode.txt"), "x").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        backing.chmod(Path::new("mode.txt"), 0o600).expect("chmod");

        let st = backing.stat(Path::new("mode.txt")).unwrap();
        assert_eq!(st.st_mode & 0o777, 0o600);
    }

    #[test]
    fn utimens_sets_times() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("time.txt"), "t").unwrap();

        let (_hold, backing) = make_backing(tmp.path());

        let atime = libc::timespec {
            tv_sec: 1_000_000,
            tv_nsec: 0,
        };
        let mtime = libc::timespec {
            tv_sec: 2_000_000,
            tv_nsec: 0,
        };
        backing
            .utimens(Path::new("time.txt"), &atime, &mtime)
            .expect("utimens");

        let st = backing.stat(Path::new("time.txt")).unwrap();
        assert_eq!(st.st_atime, 1_000_000);
        assert_eq!(st.st_mtime, 2_000_000);
    }

    #[test]
    fn access_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("exist.txt"), "a").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        backing
            .access(Path::new("exist.txt"), libc::F_OK)
            .expect("access F_OK");
    }

    #[test]
    fn access_nonexistent_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let err = backing
            .access(Path::new("nope.txt"), libc::F_OK)
            .unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
    }

    #[test]
    fn statvfs_returns_something() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let vfs = backing.statvfs().expect("statvfs");
        // Block size should be non-zero on any real filesystem.
        assert!(vfs.f_bsize > 0);
    }

    #[test]
    fn fsync_on_open_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        let fd = backing
            .open_file(Path::new("sync.txt"), libc::O_CREAT | libc::O_WRONLY, 0o644)
            .expect("open");
        backing.pwrite(fd, b"data", 0).unwrap();
        backing.fsync(fd).expect("fsync");
        backing.close_fd(fd);
    }

    #[test]
    fn readdir_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("mydir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("a.txt"), "a").unwrap();
        fs::write(sub.join("b.txt"), "b").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        let entries = backing.readdir(Path::new("mydir")).expect("readdir subdir");
        let names: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"a.txt".to_string()), "entries: {:?}", names);
        assert!(names.contains(&"b.txt".to_string()), "entries: {:?}", names);
    }

    #[test]
    fn chown_doesnt_panic() {
        // chown may fail if not root, but it shouldn't panic.
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("own.txt"), "x").unwrap();

        let (_hold, backing) = make_backing(tmp.path());
        // Pass None for both = no change, should succeed.
        let _ = backing.chown(Path::new("own.txt"), None, None);
    }

    #[test]
    fn nested_directory_operations() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, backing) = make_backing(tmp.path());

        // Create nested structure.
        backing.mkdir(Path::new("a"), 0o755).unwrap();
        backing.mkdir(Path::new("a/b"), 0o755).unwrap();
        backing.mkdir(Path::new("a/b/c"), 0o755).unwrap();

        // Create a file deep in the tree.
        let fd = backing
            .open_file(
                Path::new("a/b/c/deep.txt"),
                libc::O_CREAT | libc::O_WRONLY,
                0o644,
            )
            .unwrap();
        backing.pwrite(fd, b"deep", 0).unwrap();
        backing.close_fd(fd);

        // Stat it.
        let st = backing.stat(Path::new("a/b/c/deep.txt")).unwrap();
        assert_eq!(st.st_size, 4);

        // Readdir at the nested level.
        let entries = backing.readdir(Path::new("a/b/c")).unwrap();
        let names: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["deep.txt"]);

        // Clean up in reverse.
        backing.unlink(Path::new("a/b/c/deep.txt")).unwrap();
        backing.rmdir(Path::new("a/b/c")).unwrap();
        backing.rmdir(Path::new("a/b")).unwrap();
        backing.rmdir(Path::new("a")).unwrap();
    }
}
