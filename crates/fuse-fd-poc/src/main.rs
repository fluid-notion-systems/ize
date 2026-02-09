//! # FUSE fd-based Passthrough Proof-of-Concept
//!
//! **Problem**: When a FUSE filesystem is mounted *over* the same directory it
//! reads from (the "passthrough" or "overlay" pattern), any path-based syscall
//! the FUSE daemon makes against that directory re-enters the FUSE layer,
//! causing deadlock.
//!
//! **Solution (option 2)**: Open a directory file descriptor (`O_RDONLY |
//! O_DIRECTORY`) to the target directory *before* mounting FUSE. The kernel
//! resolves the fd to the underlying inode at `open()` time.  Subsequent
//! `*at()` syscalls (`openat`, `fstatat`, `mkdirat`, `unlinkat`, …) that
//! reference that fd operate on the **underlying** filesystem — they never
//! traverse the FUSE mount.
//!
//! This binary:
//!   1. Creates a scratch directory with seed files.
//!   2. Opens an `O_PATH` fd to it (the "base fd").
//!   3. Mounts a minimal FUSE passthrough FS *on top of that same directory*.
//!   4. Spawns a validation thread that exercises the mount (read, write,
//!      mkdir, readdir, unlink) and asserts correctness.
//!   5. Unmounts and reports results.
//!
//! Run with:
//!   cargo run -p fuse-fd-poc          # needs CAP_SYS_ADMIN / allow_other in /etc/fuse.conf
//!   cargo run -p fuse-fd-poc -- /tmp/my-test-dir   # use a specific directory
//!
//! You can also set RUST_LOG=debug for verbose FUSE-handler logging.

use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, io, process, thread};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use log::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TTL: Duration = Duration::from_secs(1);
const FUSE_ROOT_ID: u64 = 1;

// ---------------------------------------------------------------------------
// Helper: safe wrappers around *at() libc calls
// ---------------------------------------------------------------------------

/// Open a file/directory relative to `dirfd`.  Returns the new raw fd.
fn sys_openat(
    dirfd: RawFd,
    path: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<RawFd> {
    let fd = unsafe { libc::openat(dirfd, path.as_ptr(), flags, mode as libc::c_uint) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

/// `fstatat(dirfd, path, flags)` — stat a path relative to `dirfd`.
fn sys_fstatat(dirfd: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<libc::stat> {
    unsafe {
        let mut stat_buf: libc::stat = std::mem::zeroed();
        let rc = libc::fstatat(dirfd, path.as_ptr(), &mut stat_buf, flags);
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(stat_buf)
        }
    }
}

/// `fstat(fd)` — stat an open fd directly.
fn sys_fstat(fd: RawFd) -> io::Result<libc::stat> {
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

/// `mkdirat(dirfd, path, mode)`
fn sys_mkdirat(dirfd: RawFd, path: &CStr, mode: libc::mode_t) -> io::Result<()> {
    let rc = unsafe { libc::mkdirat(dirfd, path.as_ptr(), mode) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `unlinkat(dirfd, path, flags)`  (flags=0 for files, AT_REMOVEDIR for dirs)
fn sys_unlinkat(dirfd: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<()> {
    let rc = unsafe { libc::unlinkat(dirfd, path.as_ptr(), flags) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `renameat(olddirfd, oldpath, newdirfd, newpath)`
fn sys_renameat(
    olddirfd: RawFd,
    oldpath: &CStr,
    newdirfd: RawFd,
    newpath: &CStr,
) -> io::Result<()> {
    let rc = unsafe { libc::renameat(olddirfd, oldpath.as_ptr(), newdirfd, newpath.as_ptr()) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Read directory entries from an fd opened with `O_RDONLY | O_DIRECTORY`.
/// Returns Vec<(inode, dtype, name)>.
///
/// We use `getdents64` directly because `fdopendir` takes ownership of the fd
/// (closing it on `closedir`), which we do not want.  Instead, we `dup()` the
/// fd, then `fdopendir` the duplicate.
fn sys_readdir(dirfd: RawFd) -> io::Result<Vec<(u64, u8, OsString)>> {
    // dup so fdopendir doesn't steal our fd
    let dup_fd = unsafe { libc::dup(dirfd) };
    if dup_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Seek to beginning in case the dup'd fd inherits an offset
    unsafe { libc::lseek(dup_fd, 0, libc::SEEK_SET) };

    let dirp = unsafe { libc::fdopendir(dup_fd) };
    if dirp.is_null() {
        unsafe { libc::close(dup_fd) };
        return Err(io::Error::last_os_error());
    }

    let mut entries = Vec::new();
    loop {
        // Reset errno so we can distinguish EOF from error
        unsafe { *libc::__errno_location() = 0 };
        let ent = unsafe { libc::readdir(dirp) };
        if ent.is_null() {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(0) {
                break; // end of directory
            }
            unsafe { libc::closedir(dirp) };
            return Err(e);
        }
        let ent = unsafe { &*ent };
        let name_cstr = unsafe { CStr::from_ptr(ent.d_name.as_ptr()) };
        let name = OsString::from_vec(name_cstr.to_bytes().to_vec());
        entries.push((ent.d_ino, ent.d_type, name));
    }
    unsafe { libc::closedir(dirp) }; // this also closes dup_fd
    Ok(entries)
}

fn cstring_from_osstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interior nul byte"))
}

/// Build a relative CString path for a nested path (e.g. "a/b/c").
/// If the path is empty (root), returns ".".
fn rel_cpath(rel: &Path) -> io::Result<CString> {
    let bytes = rel.as_os_str().as_bytes();
    if bytes.is_empty() {
        CString::new(".").map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    } else {
        CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interior nul"))
    }
}

// ---------------------------------------------------------------------------
// FdPassthroughFS — the FUSE filesystem
// ---------------------------------------------------------------------------

/// Holds an open file for a FUSE file-handle.
struct OpenFile {
    fd: RawFd,
    #[allow(dead_code)]
    rel_path: PathBuf,
    flags: i32,
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// A minimal passthrough FUSE filesystem that performs **all** underlying I/O
/// through a pre-opened directory fd and `*at()` syscalls.
///
/// Because the base fd was opened *before* the FUSE mount, the kernel resolves
/// it to the underlying filesystem's inode — `*at()` calls never re-enter FUSE.
pub struct FdPassthroughFS {
    /// The pre-opened fd for the directory we are mounted over.
    base_fd: RawFd,

    /// inode → relative path within the mounted directory.
    inode_to_path: Mutex<HashMap<u64, PathBuf>>,

    /// Monotonic file-handle counter.
    next_fh: AtomicU64,

    /// fh → OpenFile mapping.
    open_files: Mutex<HashMap<u64, OpenFile>>,

    /// The mount point (for informational purposes only — never used for I/O).
    mount_point: PathBuf,
}

impl FdPassthroughFS {
    /// Create a new fd-based passthrough filesystem.
    ///
    /// `base_fd` must be a valid file descriptor opened with at least
    /// `O_RDONLY | O_DIRECTORY` **before** the FUSE mount is established.
    pub fn new(base_fd: RawFd, mount_point: PathBuf) -> Self {
        let mut map = HashMap::new();
        map.insert(FUSE_ROOT_ID, PathBuf::new());
        Self {
            base_fd,
            inode_to_path: Mutex::new(map),
            next_fh: AtomicU64::new(1),
            open_files: Mutex::new(HashMap::new()),
            mount_point,
        }
    }

    // -- helpers --

    fn register_inode(&self, ino: u64, rel_path: PathBuf) {
        self.inode_to_path.lock().unwrap().insert(ino, rel_path);
    }

    fn get_rel_path(&self, ino: u64) -> Option<PathBuf> {
        self.inode_to_path.lock().unwrap().get(&ino).cloned()
    }

    fn alloc_fh(&self) -> u64 {
        self.next_fh.fetch_add(1, Ordering::Relaxed)
    }

    /// Open a **directory** fd relative to `self.base_fd` for a given relative
    /// path.  Used by readdir to enumerate children.
    fn open_rel_dir(&self, rel: &Path) -> io::Result<RawFd> {
        let c = rel_cpath(rel)?;
        sys_openat(
            self.base_fd,
            &c,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            0,
        )
    }

    /// Stat a path relative to `self.base_fd`.
    fn stat_rel(&self, rel: &Path) -> io::Result<libc::stat> {
        if rel.as_os_str().is_empty() {
            // Root — fstat the base fd itself.
            sys_fstat(self.base_fd)
        } else {
            let c = rel_cpath(rel)?;
            sys_fstatat(self.base_fd, &c, libc::AT_SYMLINK_NOFOLLOW)
        }
    }

    fn stat_to_attr(st: &libc::stat, ino: u64) -> FileAttr {
        let kind = match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => FileType::Directory,
            libc::S_IFREG => FileType::RegularFile,
            libc::S_IFLNK => FileType::Symlink,
            libc::S_IFBLK => FileType::BlockDevice,
            libc::S_IFCHR => FileType::CharDevice,
            libc::S_IFIFO => FileType::NamedPipe,
            libc::S_IFSOCK => FileType::Socket,
            _ => FileType::RegularFile,
        };
        FileAttr {
            ino,
            size: st.st_size as u64,
            blocks: st.st_blocks as u64,
            atime: UNIX_EPOCH + Duration::new(st.st_atime as u64, st.st_atime_nsec as u32),
            mtime: UNIX_EPOCH + Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32),
            ctime: UNIX_EPOCH + Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32),
            crtime: UNIX_EPOCH,
            kind,
            perm: (st.st_mode & 0o7777) as u16,
            nlink: st.st_nlink as u32,
            uid: st.st_uid,
            gid: st.st_gid,
            rdev: st.st_rdev as u32,
            blksize: st.st_blksize as u32,
            flags: 0,
        }
    }

    fn dtype_to_filetype(d_type: u8) -> FileType {
        match d_type {
            libc::DT_DIR => FileType::Directory,
            libc::DT_REG => FileType::RegularFile,
            libc::DT_LNK => FileType::Symlink,
            libc::DT_BLK => FileType::BlockDevice,
            libc::DT_CHR => FileType::CharDevice,
            libc::DT_FIFO => FileType::NamedPipe,
            libc::DT_SOCK => FileType::Socket,
            _ => FileType::RegularFile,
        }
    }
}

impl Filesystem for FdPassthroughFS {
    fn init(
        &mut self,
        _req: &Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        info!(
            "FdPassthroughFS initialised — base_fd={}, mount_point={:?}",
            self.base_fd, self.mount_point
        );
        Ok(())
    }

    fn destroy(&mut self) {
        info!("FdPassthroughFS destroyed");
        // We intentionally do NOT close base_fd here — the caller owns it.
    }

    // -- lookup --

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent, name);

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let st = match self.stat_rel(&child_rel) {
            Ok(st) => st,
            Err(e) => {
                debug!("lookup: stat failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::ENOENT));
                return;
            }
        };

        let ino = st.st_ino;
        self.register_inode(ino, child_rel);
        let attr = Self::stat_to_attr(&st, ino);
        reply.entry(&TTL, &attr, 0);
    }

    // -- getattr --

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino);

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match self.stat_rel(&rel) {
            Ok(st) => {
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    st.st_ino
                };
                reply.attr(&TTL, &Self::stat_to_attr(&st, returned_ino));
            }
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- readdir --

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir(ino={}, offset={})", ino, offset);

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let dir_fd = match self.open_rel_dir(&rel) {
            Ok(fd) => fd,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        };

        let entries = match sys_readdir(dir_fd) {
            Ok(e) => {
                unsafe { libc::close(dir_fd) };
                e
            }
            Err(e) => {
                unsafe { libc::close(dir_fd) };
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        };

        // Build full entry list with stable offsets
        let mut all: Vec<(u64, FileType, OsString)> = Vec::new();
        for (entry_ino, dtype, name) in &entries {
            let ft = Self::dtype_to_filetype(*dtype);

            // Register inode mapping for children (skip . and ..)
            let name_bytes = name.as_bytes();
            if name_bytes != b"." && name_bytes != b".." {
                let child_rel = rel.join(name);
                self.register_inode(*entry_ino, child_rel);
            }

            all.push((*entry_ino, ft, name.clone()));
        }

        for (i, (entry_ino, kind, name)) in all.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *kind, OsStr::new(name)) {
                break;
            }
        }
        reply.ok();
    }

    // -- open --

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open(ino={}, flags=0x{:x})", ino, flags);

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let c = match rel_cpath(&rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        // Strip O_CREAT — open() should not create; that's create()'s job.
        let open_flags = flags & !libc::O_CREAT;

        match sys_openat(self.base_fd, &c, open_flags | libc::O_NOFOLLOW, 0) {
            Ok(fd) => {
                let fh = self.alloc_fh();
                self.open_files.lock().unwrap().insert(
                    fh,
                    OpenFile {
                        fd,
                        rel_path: rel,
                        flags,
                    },
                );
                debug!("open: fh={} fd={}", fh, fd);
                reply.opened(fh, 0);
            }
            Err(e) => {
                error!("open: openat failed for {:?}: {}", rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- read --

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        debug!("read(fh={}, offset={}, size={})", fh, offset, size);

        let files = self.open_files.lock().unwrap();
        let ofile = match files.get(&fh) {
            Some(f) => f,
            None => {
                reply.error(libc::EBADF);
                return;
            }
        };

        let mut buf = vec![0u8; size as usize];
        let n = unsafe {
            libc::pread(
                ofile.fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                offset,
            )
        };
        if n < 0 {
            reply.error(
                io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            );
        } else {
            buf.truncate(n as usize);
            reply.data(&buf);
        }
    }

    // -- write --

    fn write(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        debug!("write(fh={}, offset={}, len={})", fh, offset, data.len());

        let files = self.open_files.lock().unwrap();
        let ofile = match files.get(&fh) {
            Some(f) => f,
            None => {
                reply.error(libc::EBADF);
                return;
            }
        };

        let n = unsafe {
            libc::pwrite(
                ofile.fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
                offset,
            )
        };
        if n < 0 {
            reply.error(
                io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            );
        } else {
            reply.written(n as u32);
        }
    }

    // -- create --

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        debug!(
            "create(parent={}, name={:?}, mode=0o{:o})",
            parent, name, mode
        );

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let c = match rel_cpath(&child_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        let open_flags = flags | libc::O_CREAT | libc::O_NOFOLLOW;
        match sys_openat(self.base_fd, &c, open_flags, mode as libc::mode_t) {
            Ok(fd) => {
                // Stat to get inode
                match sys_fstat(fd) {
                    Ok(st) => {
                        let ino = st.st_ino;
                        self.register_inode(ino, child_rel.clone());

                        let fh = self.alloc_fh();
                        self.open_files.lock().unwrap().insert(
                            fh,
                            OpenFile {
                                fd,
                                rel_path: child_rel,
                                flags,
                            },
                        );

                        let attr = Self::stat_to_attr(&st, ino);
                        reply.created(&TTL, &attr, 0, fh, 0);
                    }
                    Err(e) => {
                        unsafe { libc::close(fd) };
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    }
                }
            }
            Err(e) => {
                error!("create: openat failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- mkdir --

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        debug!(
            "mkdir(parent={}, name={:?}, mode=0o{:o})",
            parent, name, mode
        );

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let c = match rel_cpath(&child_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        if let Err(e) = sys_mkdirat(self.base_fd, &c, mode as libc::mode_t) {
            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            return;
        }

        match self.stat_rel(&child_rel) {
            Ok(st) => {
                let ino = st.st_ino;
                self.register_inode(ino, child_rel);
                reply.entry(&TTL, &Self::stat_to_attr(&st, ino), 0);
            }
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- unlink --

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent, name);

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let c = match rel_cpath(&child_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        match sys_unlinkat(self.base_fd, &c, 0) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    // -- rmdir --

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent, name);

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let c = match rel_cpath(&child_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        match sys_unlinkat(self.base_fd, &c, libc::AT_REMOVEDIR) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    // -- rename --

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        debug!(
            "rename(parent={}, name={:?}, newparent={}, newname={:?})",
            parent, name, newparent, newname
        );

        let old_parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_parent_rel = match self.get_rel_path(newparent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let old_rel = old_parent_rel.join(name);
        let new_rel = new_parent_rel.join(newname);
        let old_c = match rel_cpath(&old_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };
        let new_c = match rel_cpath(&new_rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        match sys_renameat(self.base_fd, &old_c, self.base_fd, &new_c) {
            Ok(()) => {
                // Update inode map: find the inode for the old path and re-register
                // with the new path.
                let map = self.inode_to_path.lock().unwrap();
                let maybe_ino = map.iter().find(|(_, p)| **p == old_rel).map(|(i, _)| *i);
                drop(map);
                if let Some(ino) = maybe_ino {
                    self.register_inode(ino, new_rel);
                }
                reply.ok()
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    // -- flush --

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        debug!("flush(fh={})", fh);
        let files = self.open_files.lock().unwrap();
        if let Some(ofile) = files.get(&fh) {
            let rc = unsafe { libc::fsync(ofile.fd) };
            if rc < 0 {
                reply.error(
                    io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO),
                );
                return;
            }
        }
        reply.ok();
    }

    // -- release --

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        debug!("release(fh={})", fh);
        // Removing from the map drops the OpenFile, which closes the fd.
        self.open_files.lock().unwrap().remove(&fh);
        reply.ok();
    }

    // -- access --

    fn access(&mut self, _req: &Request<'_>, ino: u64, mask: i32, reply: fuser::ReplyEmpty) {
        debug!("access(ino={}, mask=0x{:x})", ino, mask);

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let c = match rel_cpath(&rel) {
            Ok(c) => c,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                return;
            }
        };

        let rc =
            unsafe { libc::faccessat(self.base_fd, c.as_ptr(), mask, libc::AT_SYMLINK_NOFOLLOW) };
        if rc < 0 {
            reply.error(
                io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EACCES),
            );
        } else {
            reply.ok();
        }
    }

    // -- setattr (minimal: truncate support) --

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!("setattr(ino={}, size={:?}, fh={:?})", ino, size, fh);

        // Handle truncate
        if let Some(new_size) = size {
            // Prefer using an existing fh if available
            if let Some(fh_val) = fh {
                let files = self.open_files.lock().unwrap();
                if let Some(ofile) = files.get(&fh_val) {
                    let rc = unsafe { libc::ftruncate(ofile.fd, new_size as libc::off_t) };
                    if rc < 0 {
                        reply.error(
                            io::Error::last_os_error()
                                .raw_os_error()
                                .unwrap_or(libc::EIO),
                        );
                        return;
                    }
                }
            } else {
                // Open, truncate, close via the base fd
                let rel = match self.get_rel_path(ino) {
                    Some(p) => p,
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                };
                let c = match rel_cpath(&rel) {
                    Ok(c) => c,
                    Err(e) => {
                        reply.error(e.raw_os_error().unwrap_or(libc::EINVAL));
                        return;
                    }
                };
                match sys_openat(self.base_fd, &c, libc::O_WRONLY | libc::O_NOFOLLOW, 0) {
                    Ok(fd) => {
                        let rc = unsafe { libc::ftruncate(fd, new_size as libc::off_t) };
                        unsafe { libc::close(fd) };
                        if rc < 0 {
                            reply.error(
                                io::Error::last_os_error()
                                    .raw_os_error()
                                    .unwrap_or(libc::EIO),
                            );
                            return;
                        }
                    }
                    Err(e) => {
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }
                }
            }
        }

        // Return updated attrs
        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match self.stat_rel(&rel) {
            Ok(st) => {
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    st.st_ino
                };
                reply.attr(&TTL, &Self::stat_to_attr(&st, returned_ino));
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    // -- statfs --

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        debug!("statfs");
        unsafe {
            let mut stfs: libc::statvfs = std::mem::zeroed();
            let rc = libc::fstatvfs(self.base_fd, &mut stfs);
            if rc < 0 {
                reply.error(
                    io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO),
                );
            } else {
                reply.statfs(
                    stfs.f_blocks,
                    stfs.f_bfree,
                    stfs.f_bavail,
                    stfs.f_files,
                    stfs.f_ffree,
                    stfs.f_bsize as u32,
                    stfs.f_namemax as u32,
                    stfs.f_frsize as u32,
                );
            }
        }
    }

    // -- opendir / releasedir --

    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        debug!("opendir(ino={})", ino);
        // We don't really need to do anything special; readdir uses the inode.
        // Just hand back a dummy fh.
        let fh = self.alloc_fh();
        reply.opened(fh, 0);
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }
}

// ---------------------------------------------------------------------------
// Validation logic — exercises the mount from the "client" (user-space) side
// ---------------------------------------------------------------------------

/// Run a series of filesystem operations through the FUSE mount and check
/// correctness.  Returns Ok(()) on success, Err(msg) on failure.
fn validate_mount(mount_point: &Path) -> Result<(), String> {
    info!("=== Starting validation against {:?} ===", mount_point);

    // Give FUSE a moment to initialise
    thread::sleep(Duration::from_millis(500));

    // 1. Read a pre-seeded file
    let seed_path = mount_point.join("seed.txt");
    info!("[1/7] Reading seed file through FUSE mount...");
    let content = fs::read_to_string(&seed_path).map_err(|e| format!("read seed.txt: {}", e))?;
    if content != "hello from before the mount\n" {
        return Err(format!("seed.txt content mismatch: got {:?}", content));
    }
    info!("  OK — seed.txt content matches");

    // 2. Write a new file
    let new_path = mount_point.join("created_via_fuse.txt");
    info!("[2/7] Creating a new file through FUSE mount...");
    fs::write(&new_path, "written through FUSE\n")
        .map_err(|e| format!("write created_via_fuse.txt: {}", e))?;
    info!("  OK — file written");

    // 3. Read it back
    info!("[3/7] Reading back the created file...");
    let readback = fs::read_to_string(&new_path)
        .map_err(|e| format!("readback created_via_fuse.txt: {}", e))?;
    if readback != "written through FUSE\n" {
        return Err(format!(
            "created_via_fuse.txt content mismatch: got {:?}",
            readback
        ));
    }
    info!("  OK — readback matches");

    // 4. Create a subdirectory
    let sub_dir = mount_point.join("subdir_via_fuse");
    info!("[4/7] Creating a subdirectory through FUSE mount...");
    fs::create_dir(&sub_dir).map_err(|e| format!("mkdir subdir_via_fuse: {}", e))?;
    info!("  OK — directory created");

    // 5. Write a file inside the subdirectory
    let sub_file = sub_dir.join("nested.txt");
    info!("[5/7] Writing a nested file...");
    fs::write(&sub_file, "nested content\n").map_err(|e| format!("write nested.txt: {}", e))?;
    let nested_content =
        fs::read_to_string(&sub_file).map_err(|e| format!("read nested.txt: {}", e))?;
    if nested_content != "nested content\n" {
        return Err(format!("nested.txt mismatch: got {:?}", nested_content));
    }
    info!("  OK — nested file written and read back");

    // 6. List the root directory
    info!("[6/7] Listing root directory through FUSE mount...");
    let entries: Vec<String> = fs::read_dir(mount_point)
        .map_err(|e| format!("readdir root: {}", e))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    info!("  entries: {:?}", entries);
    for expected in &["seed.txt", "created_via_fuse.txt", "subdir_via_fuse"] {
        if !entries.iter().any(|e| e == expected) {
            return Err(format!("readdir: missing expected entry {:?}", expected));
        }
    }
    info!("  OK — all expected entries present");

    // 7. Unlink the created file
    info!("[7/7] Removing the created file through FUSE mount...");
    fs::remove_file(&new_path).map_err(|e| format!("unlink created_via_fuse.txt: {}", e))?;
    if new_path.exists() {
        return Err("created_via_fuse.txt still exists after unlink".into());
    }
    info!("  OK — file removed");

    // Cleanup: remove nested file and subdir
    let _ = fs::remove_file(&sub_file);
    let _ = fs::remove_dir(&sub_dir);

    info!("=== All validation checks passed! ===");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // -----------------------------------------------------------------------
    // 1. Set up the target directory
    // -----------------------------------------------------------------------

    let target_dir: PathBuf = if let Some(arg) = env::args().nth(1) {
        let p = PathBuf::from(arg);
        if !p.exists() {
            fs::create_dir_all(&p).expect("failed to create target directory");
        }
        p
    } else {
        let tmp = env::temp_dir().join("fuse-fd-poc");
        if tmp.exists() {
            // Clean up any stale mount (best-effort)
            let _ = std::process::Command::new("fusermount")
                .arg("-uz")
                .arg(&tmp)
                .status();
            thread::sleep(Duration::from_millis(200));
            let _ = fs::remove_dir_all(&tmp);
        }
        fs::create_dir_all(&tmp).expect("failed to create temp dir");
        tmp
    };

    let target_dir = fs::canonicalize(&target_dir).expect("failed to canonicalize target dir");
    info!("Target directory: {:?}", target_dir);

    // Seed some content
    let seed_file = target_dir.join("seed.txt");
    fs::write(&seed_file, "hello from before the mount\n").expect("failed to write seed file");
    info!("Seeded {:?}", seed_file);

    // -----------------------------------------------------------------------
    // 2. Open the directory fd BEFORE mounting — this is the critical step!
    // -----------------------------------------------------------------------

    let dir_cpath =
        CString::new(target_dir.as_os_str().as_bytes()).expect("target dir contains nul");
    let base_fd = unsafe { libc::open(dir_cpath.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if base_fd < 0 {
        eprintln!(
            "ERROR: failed to open base directory fd: {}",
            io::Error::last_os_error()
        );
        process::exit(1);
    }
    info!(
        "Opened base directory fd={} for {:?} (BEFORE mount)",
        base_fd, target_dir
    );

    // Quick sanity check: we can fstat through the fd
    match sys_fstat(base_fd) {
        Ok(st) => info!(
            "  fstat(base_fd): ino={}, mode=0o{:o}, size={}",
            st.st_ino,
            st.st_mode & 0o7777,
            st.st_size
        ),
        Err(e) => {
            eprintln!("ERROR: fstat on base_fd failed: {}", e);
            process::exit(1);
        }
    }

    // Verify we can read through the fd with openat before mounting
    {
        let c = CString::new("seed.txt").unwrap();
        match sys_openat(base_fd, &c, libc::O_RDONLY, 0) {
            Ok(fd) => {
                let mut buf = [0u8; 64];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                unsafe { libc::close(fd) };
                if n > 0 {
                    let s = String::from_utf8_lossy(&buf[..n as usize]);
                    info!("  Pre-mount openat read: {:?} (OK)", s.trim());
                }
            }
            Err(e) => {
                eprintln!("ERROR: pre-mount openat failed: {}", e);
                process::exit(1);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 3. Mount FUSE on the same directory
    // -----------------------------------------------------------------------

    let mount_point = target_dir.clone();
    let fs = FdPassthroughFS::new(base_fd, mount_point.clone());

    let options = vec![
        MountOption::FSName("fuse-fd-poc".to_string()),
        // NOTE: We intentionally omit AutoUnmount because fuser automatically
        // adds allow_other when AutoUnmount is set, and allow_other requires
        // 'user_allow_other' in /etc/fuse.conf.  Instead we rely on the
        // BackgroundSession guard's Drop impl to unmount cleanly.
        //
        // default_permissions lets the kernel handle permission checks based
        // on the attrs we return, simplifying our implementation.
        MountOption::DefaultPermissions,
    ];

    info!("Mounting FUSE on {:?} ...", mount_point);
    info!(
        "  (base_fd={} was opened BEFORE this mount — *at() calls bypass FUSE)",
        base_fd
    );

    // Spawn the FUSE session in a background thread.  `fuser::spawn_mount2`
    // returns a guard that unmounts on drop.
    let mount_point_clone = mount_point.clone();
    let guard = match fuser::spawn_mount2(fs, &mount_point, &options) {
        Ok(guard) => {
            info!("FUSE mount established on {:?}", mount_point_clone);
            guard
        }
        Err(e) => {
            eprintln!("ERROR: failed to mount FUSE: {}", e);
            eprintln!();
            eprintln!("Hints:");
            eprintln!("  • You may need to run as root, or:");
            eprintln!("  • Add 'user_allow_other' to /etc/fuse.conf");
            eprintln!("  • Ensure the 'fuse' kernel module is loaded (modprobe fuse)");
            unsafe { libc::close(base_fd) };
            process::exit(1);
        }
    };

    // -----------------------------------------------------------------------
    // 4. Validate: exercise the mount from user-space
    // -----------------------------------------------------------------------

    // Small delay to let FUSE finish init
    thread::sleep(Duration::from_millis(300));

    // Also verify the fd is still valid and points to the UNDERLYING fs
    info!("Verifying base_fd still resolves to underlying FS after mount...");
    {
        let c = CString::new("seed.txt").unwrap();
        match sys_openat(base_fd, &c, libc::O_RDONLY, 0) {
            Ok(fd) => {
                let mut buf = [0u8; 64];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                unsafe { libc::close(fd) };
                if n > 0 {
                    let s = String::from_utf8_lossy(&buf[..n as usize]);
                    info!(
                        "  Post-mount openat(base_fd, \"seed.txt\") read: {:?} (OK — no deadlock!)",
                        s.trim()
                    );
                } else {
                    warn!("  Post-mount openat read returned 0 bytes");
                }
            }
            Err(e) => {
                // This would indicate the fd approach isn't working
                eprintln!("ERROR: post-mount openat on base_fd failed: {} — fd approach may not work on this kernel", e);
            }
        }
    }

    let result = validate_mount(&mount_point);

    // -----------------------------------------------------------------------
    // 5. Cleanup
    // -----------------------------------------------------------------------

    info!("Dropping FUSE mount guard (unmounting)...");
    drop(guard);

    // Close the base fd
    unsafe { libc::close(base_fd) };
    info!("Closed base_fd={}", base_fd);

    // Clean up temp directory contents
    let _ = fs::remove_file(target_dir.join("seed.txt"));
    // Try to remove the temp dir itself only if we created it
    if target_dir.starts_with(env::temp_dir()) {
        let _ = fs::remove_dir_all(&target_dir);
    }

    match result {
        Ok(()) => {
            println!();
            println!("============================================================");
            println!("  SUCCESS: fd-based FUSE passthrough works correctly!");
            println!();
            println!("  The pre-opened directory fd (opened BEFORE mount) allows");
            println!("  *at() syscalls to bypass the FUSE layer entirely,");
            println!("  avoiding recursive re-entry and deadlock.");
            println!();
            println!("  This validates approach #2 for ize's overlay mount");
            println!("  strategy over versioned repositories.");
            println!("============================================================");
            println!();
        }
        Err(msg) => {
            eprintln!();
            eprintln!("============================================================");
            eprintln!("  FAILURE: {}", msg);
            eprintln!("============================================================");
            eprintln!();
            process::exit(1);
        }
    }
}
