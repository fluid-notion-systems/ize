//! fd-based passthrough FUSE filesystem.
//!
//! [`FdPassthroughFS`] implements the fuser [`Filesystem`] trait by delegating
//! all underlying I/O through a [`BackingFs`] implementation.  Because the
//! backing implementation (e.g. [`LibcBackingFs`](crate::backing_fs::LibcBackingFs))
//! operates against a pre-opened directory fd, the FUSE layer never re-enters
//! itself — eliminating the deadlock that plagues naive passthrough mounts.
//!
//! # Compatibility
//!
//! `FdPassthroughFS` is a drop-in replacement for [`PassthroughFS`](super::PassthroughFS)
//! inside [`ObservingFS`](super::ObservingFS).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use log::{debug, error, info, warn};

use crate::backing_fs::{BackingFs, DirEntry};
use crate::vcs::IgnoreFilter;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// TTL for cached attributes (1 second).
const TTL: Duration = Duration::from_secs(1);

/// FUSE always uses inode 1 for the root directory.
const FUSE_ROOT_ID: u64 = 1;

// ---------------------------------------------------------------------------
// InodeMap
// ---------------------------------------------------------------------------

/// Shared inode-to-relative-path mapping.
///
/// This can be shared with observers for path resolution.
pub type InodeMap = Arc<RwLock<HashMap<u64, PathBuf>>>;

// ---------------------------------------------------------------------------
// OpenFile
// ---------------------------------------------------------------------------

/// An open file tracked by the FUSE file-handle table.
///
/// Unlike the safe-Rust `PassthroughFS` that stores `std::fs::File` objects,
/// here we store raw fds obtained from [`BackingFs::open_file`].  The fd is
/// **not** closed on drop — it must be explicitly closed via
/// [`BackingFs::close_fd`] in the `release()` handler.
struct OpenFile {
    /// Raw file descriptor from [`BackingFs::open_file`].
    fd: RawFd,
    /// Relative path within the backing store (useful for re-stat, debugging).
    #[allow(dead_code)]
    rel_path: PathBuf,
    /// Flags used when the file was opened (e.g. `O_RDONLY`, `O_RDWR`).
    flags: i32,
}

// ---------------------------------------------------------------------------
// FdPassthroughFS
// ---------------------------------------------------------------------------

/// A FUSE passthrough filesystem backed by a [`BackingFs`] implementation.
///
/// All underlying I/O is performed through `self.backing`, which typically
/// holds a pre-opened directory fd.  This design prevents FUSE re-entry
/// deadlocks when the FUSE mount overlays the same directory it reads from.
///
/// # Inode mapping
///
/// Real inodes from the underlying filesystem are used directly.  A lazily-
/// populated `inode → relative path` map is maintained so that FUSE inode-
/// based callbacks can resolve the corresponding path for `*at()` syscalls.
/// FUSE root inode 1 always maps to the empty relative path (backing root).
///
/// # File handle table
///
/// A monotonically increasing counter produces FUSE file handles.  Each handle
/// maps to an [`OpenFile`] that holds the raw fd from `BackingFs::open_file`.
/// On `release()`, the fd is closed via `BackingFs::close_fd` and removed
/// from the table.
pub struct FdPassthroughFS<B: BackingFs> {
    /// The backing filesystem that performs actual I/O.
    backing: B,

    /// Inode → relative path within the backing store.
    ///
    /// Wrapped in `Arc` so it can be shared with observers for path
    /// resolution.
    inode_to_path: InodeMap,

    /// Monotonically increasing file-handle counter.
    next_fh: AtomicU64,

    /// File-handle → [`OpenFile`] mapping.
    open_files: RwLock<HashMap<u64, OpenFile>>,

    /// When `true`, all mutating operations return `EROFS`.
    read_only: bool,

    /// Ignore filters detected at the backing root (e.g. .git, .jj, .pijul).
    ignore_filters: Vec<Box<dyn IgnoreFilter>>,

    /// The mount point path (informational only — never used for I/O).
    mount_point: PathBuf,
}

impl<B: BackingFs> FdPassthroughFS<B> {
    /// Create a new fd-based passthrough filesystem.
    ///
    /// The constructor detects ignore filters (VCS directories, etc.)
    /// present in the backing root.
    ///
    /// # Arguments
    ///
    /// * `backing` — The [`BackingFs`] implementation (typically a
    ///   [`LibcBackingFs`](crate::backing_fs::LibcBackingFs)).
    /// * `mount_point` — Where the FUSE filesystem will be mounted.  Used
    ///   only for logging and informational purposes.
    pub fn new(backing: B, mount_point: PathBuf) -> Self {
        Self::with_ignore_filters(backing, mount_point, None)
    }

    /// Create a new fd-based passthrough filesystem with specific ignore filters.
    ///
    /// If `filters` is `None`, detection is performed automatically.
    /// If provided, the given filters are used instead.
    ///
    /// # Arguments
    ///
    /// * `backing` — The [`BackingFs`] implementation
    /// * `mount_point` — Where the FUSE filesystem will be mounted
    /// * `filters` — Optional pre-detected ignore filters
    pub fn with_ignore_filters(
        backing: B,
        mount_point: PathBuf,
        filters: Option<Vec<Box<dyn IgnoreFilter>>>,
    ) -> Self {
        let mut inode_to_path = HashMap::new();
        // FUSE root inode always maps to the empty relative path.
        inode_to_path.insert(FUSE_ROOT_ID, PathBuf::new());

        // Detect ignore filters or use provided ones.
        let ignore_filters = filters.unwrap_or_else(|| Self::detect_ignore_filters(&backing));

        let filter_names: Vec<&str> = ignore_filters.iter().map(|f| f.name()).collect();
        info!(
            "FdPassthroughFS created — mount_point={:?}, detected ignore filters: {:?}",
            mount_point, filter_names
        );

        Self {
            backing,
            inode_to_path: Arc::new(RwLock::new(inode_to_path)),
            next_fh: AtomicU64::new(1),
            open_files: RwLock::new(HashMap::new()),
            read_only: false,
            ignore_filters,
            mount_point,
        }
    }

    /// Detect ignore filters at the backing root.
    ///
    /// Scans for known VCS directories and other managed directories.
    fn detect_ignore_filters(backing: &B) -> Vec<Box<dyn IgnoreFilter>> {
        // For VCS detection, we need an actual filesystem path.
        // Since BackingFs doesn't expose the base path directly, we use a workaround:
        // try to read the backing root and check for VCS directories manually.

        use crate::vcs::{GitBackend, JujutsuBackend, PijulBackend};

        let mut filters: Vec<Box<dyn IgnoreFilter>> = Vec::new();

        if let Ok(entries) = backing.readdir(Path::new("")) {
            let vcs_dirs: Vec<&str> = entries
                .iter()
                .filter(|e| e.dtype == libc::DT_DIR)
                .filter_map(|e| e.name.to_str())
                .collect();

            // Check each VCS backend
            if vcs_dirs.contains(&".git") {
                filters.push(Box::new(GitBackend));
            }
            if vcs_dirs.contains(&".jj") {
                filters.push(Box::new(JujutsuBackend));
            }
            if vcs_dirs.contains(&".pijul") {
                filters.push(Box::new(PijulBackend));
            }
        }

        filters
    }

    // -- Public accessors ---------------------------------------------------

    /// Check whether `rel_path` should be ignored by any active filter.
    ///
    /// Returns `true` if any ignore filter matches this path.
    pub fn is_ignored_path(&self, rel_path: &Path) -> bool {
        self.ignore_filters
            .iter()
            .any(|filter| filter.should_ignore(rel_path))
    }

    /// Backward-compatible alias for `is_ignored_path`.
    pub fn is_vcs_path(&self, rel_path: &Path) -> bool {
        self.is_ignored_path(rel_path)
    }

    /// Return the ignore filters detected at the backing root.
    pub fn detected_ignore_filters(&self) -> &[Box<dyn IgnoreFilter>] {
        &self.ignore_filters
    }

    /// Backward-compatible alias for `detected_ignore_filters`.
    pub fn detected_vcs_backends(&self) -> &[Box<dyn IgnoreFilter>] {
        &self.ignore_filters
    }

    /// Return the names of detected VCS/ignored directories.
    pub fn detected_vcs(&self) -> Vec<String> {
        self.ignore_filters
            .iter()
            .map(|f| f.dir_name().to_string())
            .collect()
    }

    /// Get a clone of the [`InodeMap`] for sharing with observers.
    pub fn inode_map(&self) -> InodeMap {
        Arc::clone(&self.inode_to_path)
    }

    /// Get the mount point.
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    /// Set read-only mode.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Mount the filesystem.
    pub fn mount(self) -> std::io::Result<()> {
        let mut options = vec![
            MountOption::FSName("FdPassthroughFS".to_string()),
            MountOption::AutoUnmount,
            MountOption::AllowOther,
        ];
        if self.read_only {
            options.push(MountOption::RO);
        }
        let mount_point = self.mount_point.clone();
        fuser::mount2(self, mount_point, &options)?;
        Ok(())
    }

    /// Resolve an inode to its relative path (public, for observers).
    pub fn resolve_inode(&self, ino: u64) -> Option<PathBuf> {
        self.get_rel_path(ino)
    }

    /// Resolve a path given a parent inode and a child name (for observers).
    pub fn resolve_with_name(&self, parent_ino: u64, name: &OsStr) -> Option<PathBuf> {
        self.get_rel_path(parent_ino).map(|p| p.join(name))
    }

    // -- Internal helpers ---------------------------------------------------

    /// Register an inode → relative-path mapping.
    fn register_inode(&self, ino: u64, rel_path: PathBuf) {
        self.inode_to_path.write().unwrap().insert(ino, rel_path);
    }

    /// Look up the relative path for an inode.
    fn get_rel_path(&self, ino: u64) -> Option<PathBuf> {
        self.inode_to_path.read().unwrap().get(&ino).cloned()
    }

    /// Allocate a new monotonically-increasing file handle.
    fn alloc_fh(&self) -> u64 {
        self.next_fh.fetch_add(1, Ordering::Relaxed)
    }

    // -- Attribute helpers --------------------------------------------------

    /// Convert a `libc::stat` buffer into a fuser [`FileAttr`].
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
            crtime: UNIX_EPOCH, // not available on Linux
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

    /// Map a `d_type` byte from `readdir(3)` to a fuser [`FileType`].
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

// ---------------------------------------------------------------------------
// Filesystem implementation
// ---------------------------------------------------------------------------

impl<B: BackingFs> Filesystem for FdPassthroughFS<B> {
    fn init(
        &mut self,
        _req: &Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        info!(
            "FdPassthroughFS initialised — mount_point={:?}",
            self.mount_point
        );
        Ok(())
    }

    fn destroy(&mut self) {
        info!("FdPassthroughFS destroyed");
    }

    // -- lookup -------------------------------------------------------------

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

        match self.backing.stat(&child_rel) {
            Ok(st) => {
                let ino = st.st_ino;
                self.register_inode(ino, child_rel);
                let attr = Self::stat_to_attr(&st, ino);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                debug!("lookup: stat failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::ENOENT));
            }
        }
    }

    // -- getattr ------------------------------------------------------------

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={}, fh={:?})", ino, fh);

        // Try fstat on an open fd first (cheaper, no path resolution needed).
        if let Some(fh_val) = fh {
            let handles = self.open_files.read().unwrap();
            if let Some(ofile) = handles.get(&fh_val) {
                match self.backing.fstat(ofile.fd) {
                    Ok(st) => {
                        let returned_ino = if ino == FUSE_ROOT_ID {
                            FUSE_ROOT_ID
                        } else {
                            st.st_ino
                        };
                        reply.attr(&TTL, &Self::stat_to_attr(&st, returned_ino));
                        return;
                    }
                    Err(e) => {
                        debug!(
                            "getattr: fstat on fh {} failed: {}, falling back to path",
                            fh_val, e
                        );
                    }
                }
            }
        }

        // Fall back to path-based stat.
        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                error!("getattr: inode {} not found", ino);
                reply.error(libc::ENOENT);
                return;
            }
        };

        match self.backing.stat(&rel) {
            Ok(st) => {
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    st.st_ino
                };
                reply.attr(&TTL, &Self::stat_to_attr(&st, returned_ino));
            }
            Err(e) => {
                error!("getattr: stat failed for {:?}: {}", rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- setattr ------------------------------------------------------------

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr(ino={}, mode={:?}, uid={:?}, gid={:?}, size={:?}, fh={:?})",
            ino, mode, uid, gid, size, fh
        );

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                error!("setattr: inode {} not found", ino);
                reply.error(libc::ENOENT);
                return;
            }
        };

        // -- Handle truncate (size change) --
        if let Some(new_size) = size {
            // Prefer using an existing file handle if available.
            let truncated = if let Some(fh_val) = fh {
                let handles = self.open_files.read().unwrap();
                if let Some(ofile) = handles.get(&fh_val) {
                    match self.backing.ftruncate(ofile.fd, new_size) {
                        Ok(()) => true,
                        Err(e) => {
                            error!("setattr: ftruncate via fh failed: {}", e);
                            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                            return;
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            };

            // Fall back to open-truncate-close if no fh was usable.
            if !truncated {
                match self.backing.open_file(&rel, libc::O_WRONLY, 0) {
                    Ok(fd) => {
                        let result = self.backing.ftruncate(fd, new_size);
                        self.backing.close_fd(fd);
                        if let Err(e) = result {
                            error!("setattr: ftruncate via path failed: {}", e);
                            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                            return;
                        }
                    }
                    Err(e) => {
                        error!("setattr: open for truncate failed: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }
                }
            }
        }

        // -- Handle mode change --
        if let Some(new_mode) = mode {
            if let Err(e) = self.backing.chmod(&rel, new_mode) {
                error!("setattr: chmod failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        }

        // -- Handle uid/gid change --
        if uid.is_some() || gid.is_some() {
            if let Err(e) = self.backing.chown(&rel, uid, gid) {
                error!("setattr: chown failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        }

        // -- Handle atime/mtime change --
        if atime.is_some() || mtime.is_some() {
            let to_timespec = |t: Option<TimeOrNow>| -> libc::timespec {
                match t {
                    Some(TimeOrNow::SpecificTime(st)) => {
                        let dur = st.duration_since(UNIX_EPOCH).unwrap_or_default();
                        libc::timespec {
                            tv_sec: dur.as_secs() as libc::time_t,
                            tv_nsec: dur.subsec_nanos() as libc::c_long,
                        }
                    }
                    Some(TimeOrNow::Now) => libc::timespec {
                        tv_sec: 0,
                        tv_nsec: libc::UTIME_NOW,
                    },
                    None => libc::timespec {
                        tv_sec: 0,
                        tv_nsec: libc::UTIME_OMIT,
                    },
                }
            };
            let atime_ts = to_timespec(atime);
            let mtime_ts = to_timespec(mtime);

            if let Err(e) = self.backing.utimens(&rel, &atime_ts, &mtime_ts) {
                error!("setattr: utimens failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        }

        // -- Return updated attrs --
        match self.backing.stat(&rel) {
            Ok(st) => {
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    st.st_ino
                };
                reply.attr(&TTL, &Self::stat_to_attr(&st, returned_ino));
            }
            Err(e) => {
                error!("setattr: failed to re-stat after changes: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- readdir ------------------------------------------------------------

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
                error!("readdir: inode {} not found", ino);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let entries = match self.backing.readdir(&rel) {
            Ok(e) => e,
            Err(e) => {
                error!("readdir: failed for {:?}: {}", rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
        };

        // Build the full entry list with stable offsets.
        let mut all: Vec<(u64, FileType, OsString)> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let ft = Self::dtype_to_filetype(entry.dtype);

            // Register inode mappings for children (skip . and ..).
            let name_bytes = entry.name.as_bytes();
            if name_bytes != b"." && name_bytes != b".." {
                let child_rel = rel.join(&entry.name);
                self.register_inode(entry.ino, child_rel);
            }

            all.push((entry.ino, ft, entry.name.clone()));
        }

        for (i, (entry_ino, kind, name)) in all.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *kind, OsStr::new(name)) {
                break;
            }
        }
        reply.ok();
    }

    // -- open ---------------------------------------------------------------

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open(ino={}, flags=0x{:x})", ino, flags);

        // Check read-only mode for write-ish flags.
        if self.read_only
            && (flags & (libc::O_WRONLY | libc::O_RDWR | libc::O_APPEND | libc::O_TRUNC) != 0)
        {
            reply.error(libc::EROFS);
            return;
        }

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                error!("open: inode {} not found", ino);
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Strip O_CREAT — open() should not create; that's create()'s job.
        let open_flags = flags & !libc::O_CREAT;

        match self.backing.open_file(&rel, open_flags, 0) {
            Ok(fd) => {
                let fh = self.alloc_fh();
                self.open_files.write().unwrap().insert(
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
                error!("open: failed for {:?}: {}", rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- read ---------------------------------------------------------------

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

        let handles = self.open_files.read().unwrap();
        let ofile = match handles.get(&fh) {
            Some(f) => f,
            None => {
                warn!("read: fh {} not found", fh);
                reply.error(libc::EBADF);
                return;
            }
        };

        let mut buf = vec![0u8; size as usize];
        match self.backing.pread(ofile.fd, &mut buf, offset) {
            Ok(n) => {
                buf.truncate(n);
                reply.data(&buf);
            }
            Err(e) => {
                error!("read: pread failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- write --------------------------------------------------------------

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

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let handles = self.open_files.read().unwrap();
        let ofile = match handles.get(&fh) {
            Some(f) => f,
            None => {
                warn!("write: fh {} not found", fh);
                reply.error(libc::EBADF);
                return;
            }
        };

        match self.backing.pwrite(ofile.fd, data, offset) {
            Ok(n) => {
                reply.written(n as u32);
            }
            Err(e) => {
                error!("write: pwrite failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- create -------------------------------------------------------------

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
            "create(parent={}, name={:?}, mode=0o{:o}, flags=0x{:x})",
            parent, name, mode, flags
        );

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                error!("create: parent inode {} not found", parent);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);
        let open_flags = flags | libc::O_CREAT;

        match self.backing.open_file(&child_rel, open_flags, mode) {
            Ok(fd) => {
                // Stat the newly created file to get its inode.
                match self.backing.fstat(fd) {
                    Ok(st) => {
                        let ino = st.st_ino;
                        self.register_inode(ino, child_rel.clone());

                        let fh = self.alloc_fh();
                        self.open_files.write().unwrap().insert(
                            fh,
                            OpenFile {
                                fd,
                                rel_path: child_rel,
                                flags,
                            },
                        );

                        let attr = Self::stat_to_attr(&st, ino);
                        debug!("create: ino={}, fh={}", ino, fh);
                        reply.created(&TTL, &attr, 0, fh, 0);
                    }
                    Err(e) => {
                        self.backing.close_fd(fd);
                        error!("create: fstat after create failed: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    }
                }
            }
            Err(e) => {
                error!("create: open_file failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- mkdir --------------------------------------------------------------

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

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                error!("mkdir: parent inode {} not found", parent);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);

        if let Err(e) = self.backing.mkdir(&child_rel, mode) {
            error!("mkdir: failed for {:?}: {}", child_rel, e);
            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            return;
        }

        match self.backing.stat(&child_rel) {
            Ok(st) => {
                let ino = st.st_ino;
                self.register_inode(ino, child_rel);
                reply.entry(&TTL, &Self::stat_to_attr(&st, ino), 0);
            }
            Err(e) => {
                error!("mkdir: stat after mkdir failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- unlink -------------------------------------------------------------

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent, name);

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                error!("unlink: parent inode {} not found", parent);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);

        // Grab the inode before unlinking so we can clean up the map.
        let maybe_ino = self.backing.stat(&child_rel).ok().map(|st| st.st_ino);

        match self.backing.unlink(&child_rel) {
            Ok(()) => {
                if let Some(ino) = maybe_ino {
                    self.inode_to_path.write().unwrap().remove(&ino);
                }
                reply.ok();
            }
            Err(e) => {
                error!("unlink: failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- rmdir --------------------------------------------------------------

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent, name);

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                error!("rmdir: parent inode {} not found", parent);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_rel = parent_rel.join(name);

        // Grab the inode before removing so we can clean up the map.
        let maybe_ino = self.backing.stat(&child_rel).ok().map(|st| st.st_ino);

        match self.backing.rmdir(&child_rel) {
            Ok(()) => {
                if let Some(ino) = maybe_ino {
                    self.inode_to_path.write().unwrap().remove(&ino);
                }
                reply.ok();
            }
            Err(e) => {
                error!("rmdir: failed for {:?}: {}", child_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- rename -------------------------------------------------------------

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        debug!(
            "rename(parent={}, name={:?}, newparent={}, newname={:?})",
            parent, name, newparent, newname
        );

        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let old_parent_rel = match self.get_rel_path(parent) {
            Some(p) => p,
            None => {
                error!("rename: old parent inode {} not found", parent);
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_parent_rel = match self.get_rel_path(newparent) {
            Some(p) => p,
            None => {
                error!("rename: new parent inode {} not found", newparent);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let old_rel = old_parent_rel.join(name);
        let new_rel = new_parent_rel.join(newname);

        // Get the inode of the old entry so we can update the map.
        let maybe_ino = self.backing.stat(&old_rel).ok().map(|st| st.st_ino);

        match self.backing.rename(&old_rel, &new_rel) {
            Ok(()) => {
                if let Some(ino) = maybe_ino {
                    self.register_inode(ino, new_rel);
                }
                reply.ok();
            }
            Err(e) => {
                error!("rename: failed {:?} -> {:?}: {}", old_rel, new_rel, e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- flush --------------------------------------------------------------

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        debug!("flush(fh={})", fh);

        let handles = self.open_files.read().unwrap();
        if let Some(ofile) = handles.get(&fh) {
            if let Err(e) = self.backing.fsync(ofile.fd) {
                // EBADF can happen if the fd was already closed, treat as OK.
                if e.raw_os_error() != Some(libc::EBADF) {
                    error!("flush: fsync failed: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }
            }
        }
        reply.ok();
    }

    // -- release ------------------------------------------------------------

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!("release(fh={})", fh);

        if let Some(ofile) = self.open_files.write().unwrap().remove(&fh) {
            self.backing.close_fd(ofile.fd);
        }
        reply.ok();
    }

    // -- fsync --------------------------------------------------------------

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        debug!("fsync(fh={})", fh);

        let handles = self.open_files.read().unwrap();
        match handles.get(&fh) {
            Some(ofile) => match self.backing.fsync(ofile.fd) {
                Ok(()) => reply.ok(),
                Err(e) => {
                    error!("fsync: failed: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                }
            },
            None => {
                warn!("fsync: fh {} not found", fh);
                reply.error(libc::EBADF);
            }
        }
    }

    // -- access -------------------------------------------------------------

    fn access(&mut self, _req: &Request<'_>, ino: u64, mask: i32, reply: ReplyEmpty) {
        debug!("access(ino={}, mask=0x{:x})", ino, mask);

        let rel = match self.get_rel_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match self.backing.access(&rel, mask) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EACCES)),
        }
    }

    // -- statfs -------------------------------------------------------------

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        debug!("statfs");

        match self.backing.statvfs() {
            Ok(stfs) => {
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
            Err(e) => {
                error!("statfs: failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    // -- opendir / releasedir -----------------------------------------------

    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        debug!("opendir(ino={})", ino);
        // Just verify the inode is known.
        if ino == FUSE_ROOT_ID || self.get_rel_path(ino).is_some() {
            reply.opened(0, 0);
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        debug!("releasedir");
        reply.ok();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backing_fs::LibcBackingFs;
    use std::fs;
    use std::os::unix::io::AsRawFd;

    /// Create a temp dir with a LibcBackingFs-backed FdPassthroughFS.
    fn make_fs(tmp: &std::path::Path) -> (fs::File, FdPassthroughFS<LibcBackingFs>) {
        let dir_file = fs::File::open(tmp).expect("open tmpdir");
        let backing = LibcBackingFs::from_raw_fd(dir_file.as_raw_fd());
        let fs = FdPassthroughFS::new(backing, tmp.to_path_buf());
        (dir_file, fs)
    }

    #[test]
    fn construction_and_inode_map() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        // Root inode should be pre-registered.
        assert!(fs.get_rel_path(FUSE_ROOT_ID).is_some());
        assert_eq!(fs.get_rel_path(FUSE_ROOT_ID).unwrap(), PathBuf::new());

        // Inode map should be clonable for sharing.
        let map = fs.inode_map();
        assert!(map.read().unwrap().contains_key(&FUSE_ROOT_ID));
    }

    #[test]
    fn is_vcs_path_detection() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::create_dir(tmp.path().join(".pijul")).unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();

        let (_hold, fs) = make_fs(tmp.path());

        let detected = fs.detected_vcs();
        assert_eq!(detected.len(), 2);
        assert!(detected.contains(&".git".to_string()));
        assert!(detected.contains(&".pijul".to_string()));

        assert!(fs.is_vcs_path(Path::new(".git")));
        assert!(fs.is_vcs_path(Path::new(".git/objects")));
        assert!(fs.is_vcs_path(Path::new(".pijul/config")));
        assert!(!fs.is_vcs_path(Path::new("src")));
        assert!(!fs.is_vcs_path(Path::new("src/main.rs")));
        assert!(!fs.is_vcs_path(Path::new("")));
    }

    #[test]
    fn inode_registration_and_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        fs.register_inode(42, PathBuf::from("some/path"));
        assert_eq!(fs.resolve_inode(42), Some(PathBuf::from("some/path")));
        assert_eq!(
            fs.resolve_with_name(42, OsStr::new("child.txt")),
            Some(PathBuf::from("some/path/child.txt"))
        );
        assert_eq!(fs.resolve_inode(999), None);
    }

    #[test]
    fn stat_to_attr_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        let st = fs.backing.stat(Path::new("")).unwrap();
        let attr = FdPassthroughFS::<LibcBackingFs>::stat_to_attr(&st, FUSE_ROOT_ID);
        assert_eq!(attr.ino, FUSE_ROOT_ID);
        assert_eq!(attr.kind, FileType::Directory);
    }

    #[test]
    fn stat_to_attr_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("test.txt"), "hello").unwrap();

        let (_hold, fs_inst) = make_fs(tmp.path());

        let st = fs_inst.backing.stat(Path::new("test.txt")).unwrap();
        let attr = FdPassthroughFS::<LibcBackingFs>::stat_to_attr(&st, st.st_ino);
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 5);
    }

    #[test]
    fn dtype_to_filetype_mapping() {
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_DIR),
            FileType::Directory
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_REG),
            FileType::RegularFile
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_LNK),
            FileType::Symlink
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_BLK),
            FileType::BlockDevice
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_CHR),
            FileType::CharDevice
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_FIFO),
            FileType::NamedPipe
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(libc::DT_SOCK),
            FileType::Socket
        );
        assert_eq!(
            FdPassthroughFS::<LibcBackingFs>::dtype_to_filetype(0),
            FileType::RegularFile
        );
    }

    #[test]
    fn alloc_fh_monotonic() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        let fh1 = fs.alloc_fh();
        let fh2 = fs.alloc_fh();
        let fh3 = fs.alloc_fh();
        assert!(fh1 < fh2);
        assert!(fh2 < fh3);
    }

    #[test]
    fn read_only_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, mut fs) = make_fs(tmp.path());

        assert!(!fs.read_only);
        fs.set_read_only(true);
        assert!(fs.read_only);
    }

    #[test]
    fn mount_point_accessor() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        assert_eq!(fs.mount_point(), tmp.path());
    }

    #[test]
    fn no_vcs_dirs_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (_hold, fs) = make_fs(tmp.path());

        assert_eq!(fs.detected_vcs().len(), 0);
        assert!(!fs.is_vcs_path(Path::new(".git")));
    }

    #[test]
    fn jj_vcs_detection() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(".jj")).unwrap();

        let (_hold, fs) = make_fs(tmp.path());

        let detected = fs.detected_vcs();
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&".jj".to_string()));

        assert!(fs.is_vcs_path(Path::new(".jj")));
        assert!(fs.is_vcs_path(Path::new(".jj/repo")));
    }
}
