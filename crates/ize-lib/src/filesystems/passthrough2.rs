//! PassthroughFS2 - A simplified passthrough filesystem
//!
//! This implementation differs from the original PassthroughFS in several key ways:
//! 1. Uses **real inodes** from the underlying filesystem instead of synthetic counters
//! 2. Uses **generated file handles** (not raw fds) for FUSE operations
//! 3. Has no concept of a "database file" - just source_dir ↔ mount_point
//! 4. Implements proper file descriptor lifecycle management via RAII
//! 5. Uses safe Rust APIs (FileExt, nix) instead of unsafe libc calls

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{EBADF, EIO, ENOENT, ENOTDIR, ENOTEMPTY};
use log::{debug, error, info, warn};
use nix::fcntl::AT_FDCWD;
use nix::sys::stat::{utimensat, UtimensatFlags};
use nix::sys::statvfs::statvfs;
use nix::sys::time::TimeSpec;
use nix::unistd::{chown, Gid, Uid};

/// TTL for cached attributes (1 second)
const TTL: Duration = Duration::from_secs(1);

/// FUSE root inode number
const FUSE_ROOT_ID: u64 = 1;

/// Stored file handle - keeps the File alive so fd remains valid
struct FileHandle {
    /// The File object that owns the fd - when dropped, fd is automatically closed
    file: File,
    /// The real path (useful for some operations)
    #[allow(dead_code)]
    real_path: PathBuf,
    /// Flags used when opening (O_RDONLY, O_RDWR, etc.)
    flags: i32,
}

/// A simplified passthrough filesystem implementation
///
/// Key design decisions:
/// - Uses real inodes from the underlying filesystem
/// - Uses generated file handles (not raw fds) for FUSE operations
/// - Maintains an inode → path mapping for reverse lookups
/// - File handles are stored to keep fds alive until release()
pub struct PassthroughFS2 {
    /// The source directory being exposed
    source_dir: PathBuf,
    /// The mount point
    mount_point: PathBuf,
    /// Read-only mode
    read_only: bool,
    /// Maps real inode → relative path (for inode-based lookups)
    /// Populated during lookup() and readdir()
    inode_to_path: RwLock<HashMap<u64, PathBuf>>,
    /// Next file handle to assign
    next_fh: AtomicU64,
    /// Maps fh → FileHandle (keeps File alive)
    file_handles: RwLock<HashMap<u64, FileHandle>>,
}

impl PassthroughFS2 {
    /// Create a new passthrough filesystem
    ///
    /// # Arguments
    /// * `source_dir` - The directory to expose through the filesystem
    /// * `mount_point` - Where the filesystem will be mounted
    ///
    /// # Errors
    /// Returns an error if the source directory doesn't exist or can't be accessed
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(source_dir: P, mount_point: Q) -> io::Result<Self> {
        let source_dir = source_dir.as_ref().to_path_buf();
        let mount_point = mount_point.as_ref().to_path_buf();

        // Verify source directory exists
        if !source_dir.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Source directory not found: {:?}", source_dir),
            ));
        }

        if !source_dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Source path is not a directory: {:?}", source_dir),
            ));
        }

        let mut inode_to_path = HashMap::new();
        // Register root inode mapping - FUSE always uses inode 1 for root
        inode_to_path.insert(FUSE_ROOT_ID, PathBuf::new());

        info!(
            "Initialized PassthroughFS2 with source_dir: {:?}, mount_point: {:?}",
            source_dir, mount_point
        );

        Ok(Self {
            source_dir,
            mount_point,
            read_only: false,
            inode_to_path: RwLock::new(inode_to_path),
            next_fh: AtomicU64::new(1),
            file_handles: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new read-only passthrough filesystem
    pub fn new_read_only<P: AsRef<Path>, Q: AsRef<Path>>(
        source_dir: P,
        mount_point: Q,
    ) -> io::Result<Self> {
        let mut fs = Self::new(source_dir, mount_point)?;
        fs.read_only = true;
        Ok(fs)
    }

    /// Set read-only mode
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Get the source directory
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// Get the mount point
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    /// Mount the filesystem
    pub fn mount(self) -> io::Result<()> {
        let mut options = vec![
            MountOption::FSName("PassthroughFS2".to_string()),
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

    // =========================================================================
    // Path Helpers
    // =========================================================================

    /// Convert relative path to real path in source_dir
    fn to_real(&self, rel_path: &Path) -> PathBuf {
        self.source_dir.join(rel_path)
    }

    /// Get inode for a path (uses real inode from filesystem)
    #[allow(dead_code)]
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

    /// Get the real inode for the source directory (our root)
    #[allow(dead_code)]
    fn get_root_real_inode(&self) -> io::Result<u64> {
        self.get_inode(&self.source_dir)
    }

    /// Allocate a new file handle
    fn alloc_fh(&self) -> u64 {
        self.next_fh.fetch_add(1, Ordering::SeqCst)
    }

    // =========================================================================
    // Attribute Helpers
    // =========================================================================

    /// Convert std::fs::Metadata to fuser::FileAttr
    fn metadata_to_attr(&self, meta: &fs::Metadata, ino: u64) -> FileAttr {
        let kind = if meta.is_dir() {
            FileType::Directory
        } else if meta.is_file() {
            FileType::RegularFile
        } else if meta.file_type().is_symlink() {
            FileType::Symlink
        } else if meta.file_type().is_block_device() {
            FileType::BlockDevice
        } else if meta.file_type().is_char_device() {
            FileType::CharDevice
        } else if meta.file_type().is_fifo() {
            FileType::NamedPipe
        } else if meta.file_type().is_socket() {
            FileType::Socket
        } else {
            FileType::RegularFile
        };

        FileAttr {
            ino,
            size: meta.size(),
            blocks: meta.blocks(),
            atime: UNIX_EPOCH + Duration::from_secs(meta.atime() as u64),
            mtime: UNIX_EPOCH + Duration::from_secs(meta.mtime() as u64),
            ctime: UNIX_EPOCH + Duration::from_secs(meta.ctime() as u64),
            crtime: UNIX_EPOCH, // Not available on Linux
            kind,
            perm: (meta.mode() & 0o7777) as u16,
            nlink: meta.nlink() as u32,
            uid: meta.uid(),
            gid: meta.gid(),
            rdev: meta.rdev() as u32,
            blksize: meta.blksize() as u32,
            flags: 0,
        }
    }

    /// Get file type from metadata for readdir
    fn metadata_to_filetype(meta: &fs::Metadata) -> FileType {
        if meta.is_dir() {
            FileType::Directory
        } else if meta.is_file() {
            FileType::RegularFile
        } else if meta.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::RegularFile
        }
    }

    /// Helper to truncate via path (when no valid fh is available)
    fn truncate_via_path(path: &Path, size: u64) -> io::Result<()> {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(size)
    }

    /// Convert nix error to raw os error code
    fn nix_err_to_errno(e: nix::Error) -> i32 {
        e as i32
    }
}

impl Filesystem for PassthroughFS2 {
    /// Initialize filesystem
    fn init(
        &mut self,
        _req: &Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        info!("PassthroughFS2 initialized");
        Ok(())
    }

    /// Clean up filesystem
    fn destroy(&mut self) {
        info!("PassthroughFS2 destroyed");
    }

    /// Look up a directory entry by name and get its attributes
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent, name);

        // Get parent path
        let parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("lookup: parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build child path
        let rel_path = parent_path.join(name);
        let real_path = self.to_real(&rel_path);

        debug!("lookup: checking real path {:?}", real_path);

        // Stat the file
        match fs::metadata(&real_path) {
            Ok(meta) => {
                let ino = meta.ino();

                // Register inode → path mapping
                self.register_inode(ino, rel_path);

                let attr = self.metadata_to_attr(&meta, ino);
                debug!("lookup: found inode {} for {:?}", ino, name);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                debug!("lookup: {:?} not found: {}", name, e);
                reply.error(e.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    /// Get file attributes
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino);

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("getattr: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };

        let real_path = self.to_real(&rel_path);

        match fs::metadata(&real_path) {
            Ok(meta) => {
                // Use the real inode from the filesystem, but for FUSE_ROOT_ID
                // we must return inode 1 (not the real one)
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    meta.ino()
                };
                let attr = self.metadata_to_attr(&meta, returned_ino);
                reply.attr(&TTL, &attr);
            }
            Err(e) => {
                error!("getattr: failed to stat {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    /// Set file attributes
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

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("setattr: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };
        let real_path = self.to_real(&rel_path);

        // Handle truncate (size change)
        if let Some(new_size) = size {
            let result = if let Some(fh_val) = fh {
                let handles = self.file_handles.read().unwrap();

                if let Some(handle) = handles.get(&fh_val) {
                    // Check if opened for writing
                    if (handle.flags & (libc::O_WRONLY | libc::O_RDWR)) != 0 {
                        // Use File::set_len() - safe Rust API
                        debug!("setattr: using File::set_len() on fh {}", fh_val);
                        handle.file.set_len(new_size)
                    } else {
                        // Not opened for writing, fall back to path-based
                        drop(handles);
                        Self::truncate_via_path(&real_path, new_size)
                    }
                } else {
                    drop(handles);
                    Self::truncate_via_path(&real_path, new_size)
                }
            } else {
                Self::truncate_via_path(&real_path, new_size)
            };

            if let Err(e) = result {
                error!("setattr: truncate failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
                return;
            }
        }

        // Handle mode change
        if let Some(new_mode) = mode {
            if let Err(e) = fs::set_permissions(&real_path, fs::Permissions::from_mode(new_mode)) {
                error!("setattr: chmod failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
                return;
            }
        }

        // Handle uid/gid change using nix::unistd::chown
        if uid.is_some() || gid.is_some() {
            let uid_opt = uid.map(Uid::from_raw);
            let gid_opt = gid.map(Gid::from_raw);

            if let Err(e) = chown(&real_path, uid_opt, gid_opt) {
                error!("setattr: chown failed: {}", e);
                reply.error(Self::nix_err_to_errno(e));
                return;
            }
        }

        // Handle atime/mtime change using nix::sys::stat::utimensat
        if atime.is_some() || mtime.is_some() {
            let to_timespec = |t: Option<TimeOrNow>| -> TimeSpec {
                match t {
                    Some(TimeOrNow::SpecificTime(st)) => {
                        let duration = st.duration_since(UNIX_EPOCH).unwrap_or_default();
                        TimeSpec::new(duration.as_secs() as i64, duration.subsec_nanos() as i64)
                    }
                    Some(TimeOrNow::Now) => TimeSpec::UTIME_NOW,
                    None => TimeSpec::UTIME_OMIT,
                }
            };

            let atime_ts = to_timespec(atime);
            let mtime_ts = to_timespec(mtime);

            if let Err(e) = utimensat(
                AT_FDCWD,
                &real_path,
                &atime_ts,
                &mtime_ts,
                UtimensatFlags::NoFollowSymlink,
            ) {
                error!("setattr: utimensat failed: {}", e);
                reply.error(Self::nix_err_to_errno(e));
                return;
            }
        }

        // Return updated attributes
        match fs::metadata(&real_path) {
            Ok(meta) => {
                let returned_ino = if ino == FUSE_ROOT_ID {
                    FUSE_ROOT_ID
                } else {
                    meta.ino()
                };
                let attr = self.metadata_to_attr(&meta, returned_ino);
                reply.attr(&TTL, &attr);
            }
            Err(e) => {
                error!("setattr: failed to get updated attrs: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Read directory entries
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir(ino={}, offset={})", ino, offset);

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("readdir: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };

        let real_path = self.to_real(&rel_path);

        if !real_path.is_dir() {
            reply.error(ENOTDIR);
            return;
        }

        let entries = match fs::read_dir(&real_path) {
            Ok(e) => e,
            Err(e) => {
                error!("readdir: failed to read {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(ENOENT));
                return;
            }
        };

        // Build entries list
        let mut all_entries: Vec<(u64, FileType, String)> = Vec::new();

        // Add "." entry (current directory)
        all_entries.push((ino, FileType::Directory, ".".to_string()));

        // Add ".." entry (parent directory) - simplified, just use same ino
        // In a real implementation, we'd look up the parent's inode
        all_entries.push((FUSE_ROOT_ID, FileType::Directory, "..".to_string()));

        // Add directory entries
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                let child_ino = meta.ino();
                let file_type = Self::metadata_to_filetype(&meta);
                let name = entry.file_name().to_string_lossy().into_owned();

                // Register inode mapping for each entry
                let child_rel_path = rel_path.join(&entry.file_name());
                self.register_inode(child_ino, child_rel_path);

                all_entries.push((child_ino, file_type, name));
            }
        }

        // Return entries starting from offset
        for (i, (entry_ino, kind, name)) in all_entries.iter().enumerate().skip(offset as usize) {
            // reply.add returns true if the buffer is full
            if reply.add(*entry_ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }

        reply.ok();
    }

    /// Open a file
    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open(ino={}, flags=0x{:x})", ino, flags);

        // Check read-only mode for write operations
        if self.read_only
            && (flags & (libc::O_WRONLY | libc::O_RDWR | libc::O_APPEND | libc::O_TRUNC) != 0)
        {
            reply.error(libc::EROFS);
            return;
        }

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("open: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };
        let real_path = self.to_real(&rel_path);

        // Open with appropriate flags
        let mut options = OpenOptions::new();
        let access_mode = flags & libc::O_ACCMODE;
        match access_mode {
            libc::O_RDONLY => {
                options.read(true);
            }
            libc::O_WRONLY => {
                options.write(true);
            }
            libc::O_RDWR => {
                options.read(true).write(true);
            }
            _ => {
                options.read(true);
            }
        }

        if flags & libc::O_APPEND != 0 {
            options.append(true);
        }
        if flags & libc::O_TRUNC != 0 {
            options.truncate(true);
        }

        // Pass through other flags via custom_flags
        options.custom_flags(
            flags & !(libc::O_ACCMODE | libc::O_APPEND | libc::O_TRUNC | libc::O_CREAT),
        );

        match options.open(&real_path) {
            Ok(file) => {
                // Allocate a new file handle (not the raw fd)
                let fh = self.alloc_fh();

                // Store File to keep fd alive
                let handle = FileHandle {
                    file,
                    real_path,
                    flags,
                };
                self.file_handles.write().unwrap().insert(fh, handle);

                debug!("open: opened fh {} for inode {}", fh, ino);

                // Return our generated fh as the FUSE file handle
                reply.opened(fh, 0);
            }
            Err(e) => {
                error!("open: failed to open {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Read data from a file using FileExt::read_at
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

        let handles = self.file_handles.read().unwrap();

        if let Some(handle) = handles.get(&fh) {
            let mut buf = vec![0u8; size as usize];

            // Use FileExt::read_at for thread-safe positional read
            match handle.file.read_at(&mut buf, offset as u64) {
                Ok(n) => {
                    buf.truncate(n);
                    reply.data(&buf);
                }
                Err(e) => {
                    error!("read: read_at failed: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                }
            }
        } else {
            warn!("read: fh {} not found in file_handles table", fh);
            reply.error(EBADF);
        }
    }

    /// Write data to a file using FileExt::write_at
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

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        let handles = self.file_handles.read().unwrap();

        if let Some(handle) = handles.get(&fh) {
            // Use FileExt::write_at for thread-safe positional write
            match handle.file.write_at(data, offset as u64) {
                Ok(n) => {
                    reply.written(n as u32);
                }
                Err(e) => {
                    error!("write: write_at failed: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                }
            }
        } else {
            warn!("write: fh {} not found in file_handles table", fh);
            reply.error(EBADF);
        }
    }

    /// Flush file data using File::sync_all
    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        debug!("flush(fh={})", fh);

        let handles = self.file_handles.read().unwrap();

        if let Some(handle) = handles.get(&fh) {
            // Use File::sync_all() - safe Rust API
            match handle.file.sync_all() {
                Ok(()) => reply.ok(),
                Err(e) => {
                    // EBADF is common if the file was already closed, treat it as success
                    if e.raw_os_error() == Some(EBADF) {
                        reply.ok();
                    } else {
                        error!("flush: sync_all failed: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(EIO));
                    }
                }
            }
        } else {
            // File handle not found, might already be closed - treat as success
            reply.ok();
        }
    }

    /// Release an open file
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

        // Remove from table - File is dropped, fd is automatically closed via RAII
        self.file_handles.write().unwrap().remove(&fh);

        reply.ok();
    }

    /// Synchronize file contents using File::sync_all/sync_data
    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
        debug!("fsync(fh={}, datasync={})", fh, datasync);

        let handles = self.file_handles.read().unwrap();

        if let Some(handle) = handles.get(&fh) {
            // Use File::sync_all() or File::sync_data() - safe Rust APIs
            let result = if datasync {
                handle.file.sync_data()
            } else {
                handle.file.sync_all()
            };

            match result {
                Ok(()) => reply.ok(),
                Err(e) => {
                    error!("fsync: failed: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                }
            }
        } else {
            warn!("fsync: fh {} not found", fh);
            reply.error(EBADF);
        }
    }

    /// Create and open a file
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

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get parent path
        let parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("create: parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build child path
        let rel_path = parent_path.join(name);
        let real_path = self.to_real(&rel_path);

        debug!("create: creating file at {:?}", real_path);

        // Open with O_CREAT | O_EXCL to atomically create
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        options.mode(mode);

        match options.open(&real_path) {
            Ok(file) => {
                // Get metadata for the new file
                match fs::metadata(&real_path) {
                    Ok(meta) => {
                        let ino = meta.ino();
                        let fh = self.alloc_fh();

                        // Register inode mapping
                        self.register_inode(ino, rel_path);

                        // Store file handle
                        let handle = FileHandle {
                            file,
                            real_path,
                            flags: libc::O_RDWR,
                        };
                        self.file_handles.write().unwrap().insert(fh, handle);

                        let attr = self.metadata_to_attr(&meta, ino);
                        debug!("create: created inode {} with fh {}", ino, fh);
                        reply.created(&TTL, &attr, 0, fh, 0);
                    }
                    Err(e) => {
                        error!("create: failed to stat new file: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(EIO));
                    }
                }
            }
            Err(e) => {
                error!("create: failed to create {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Create a directory
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

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get parent path
        let parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("mkdir: parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build child path
        let rel_path = parent_path.join(name);
        let real_path = self.to_real(&rel_path);

        debug!("mkdir: creating directory at {:?}", real_path);

        // Create the directory
        match fs::create_dir(&real_path) {
            Ok(()) => {
                // Set permissions
                if let Err(e) = fs::set_permissions(&real_path, fs::Permissions::from_mode(mode)) {
                    warn!("mkdir: failed to set permissions: {}", e);
                }

                // Get metadata
                match fs::metadata(&real_path) {
                    Ok(meta) => {
                        let ino = meta.ino();

                        // Register inode mapping
                        self.register_inode(ino, rel_path);

                        let attr = self.metadata_to_attr(&meta, ino);
                        debug!("mkdir: created directory with inode {}", ino);
                        reply.entry(&TTL, &attr, 0);
                    }
                    Err(e) => {
                        error!("mkdir: failed to stat new directory: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(EIO));
                    }
                }
            }
            Err(e) => {
                error!("mkdir: failed to create {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Remove a file
    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent, name);

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get parent path
        let parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("unlink: parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build child path
        let rel_path = parent_path.join(name);
        let real_path = self.to_real(&rel_path);

        debug!("unlink: removing file at {:?}", real_path);

        // Get inode before removing (for cleanup)
        let ino = fs::metadata(&real_path).ok().map(|m| m.ino());

        // Remove the file
        match fs::remove_file(&real_path) {
            Ok(()) => {
                // Remove from inode map
                if let Some(ino) = ino {
                    self.inode_to_path.write().unwrap().remove(&ino);
                }
                debug!("unlink: removed file");
                reply.ok();
            }
            Err(e) => {
                error!("unlink: failed to remove {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Remove a directory
    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent, name);

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get parent path
        let parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("rmdir: parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build child path
        let rel_path = parent_path.join(name);
        let real_path = self.to_real(&rel_path);

        debug!("rmdir: removing directory at {:?}", real_path);

        // Get inode before removing (for cleanup)
        let ino = fs::metadata(&real_path).ok().map(|m| m.ino());

        // Remove the directory
        match fs::remove_dir(&real_path) {
            Ok(()) => {
                // Remove from inode map
                if let Some(ino) = ino {
                    self.inode_to_path.write().unwrap().remove(&ino);
                }
                debug!("rmdir: removed directory");
                reply.ok();
            }
            Err(e) => {
                error!("rmdir: failed to remove {:?}: {}", real_path, e);
                // Check for ENOTEMPTY
                if e.kind() == io::ErrorKind::DirectoryNotEmpty {
                    reply.error(ENOTEMPTY);
                } else {
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                }
            }
        }
    }

    /// Rename a file or directory
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

        // Check read-only mode
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get old parent path
        let old_parent_path = if parent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => p,
                None => {
                    error!("rename: old parent inode {} not found", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Get new parent path
        let new_parent_path = if newparent == FUSE_ROOT_ID {
            PathBuf::new()
        } else {
            match self.get_path_for_inode(newparent) {
                Some(p) => p,
                None => {
                    error!("rename: new parent inode {} not found", newparent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        // Build paths
        let old_rel_path = old_parent_path.join(name);
        let new_rel_path = new_parent_path.join(newname);
        let old_real_path = self.to_real(&old_rel_path);
        let new_real_path = self.to_real(&new_rel_path);

        debug!("rename: {:?} -> {:?}", old_real_path, new_real_path);

        // Get inode before rename (for updating mapping)
        let ino = fs::metadata(&old_real_path).ok().map(|m| m.ino());

        // Perform the rename
        match fs::rename(&old_real_path, &new_real_path) {
            Ok(()) => {
                // Update inode mapping with new path
                if let Some(ino) = ino {
                    self.register_inode(ino, new_rel_path);
                }
                debug!("rename: completed successfully");
                reply.ok();
            }
            Err(e) => {
                error!("rename: failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Open a directory
    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        debug!("opendir(ino={})", ino);
        // We don't need to track directory handles, just verify the inode exists
        if ino == FUSE_ROOT_ID || self.get_path_for_inode(ino).is_some() {
            reply.opened(0, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    /// Release an open directory
    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        debug!("releasedir()");
        reply.ok();
    }

    /// Check file access permissions using nix::unistd::access
    fn access(&mut self, _req: &Request<'_>, ino: u64, mask: i32, reply: ReplyEmpty) {
        debug!("access(ino={}, mask=0x{:x})", ino, mask);

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let real_path = self.to_real(&rel_path);

        // Use nix::unistd::access for safe access check
        use nix::unistd::{access, AccessFlags};

        let flags = AccessFlags::from_bits_truncate(mask);

        match access(&real_path, flags) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Self::nix_err_to_errno(e)),
        }
    }

    /// Get filesystem statistics using nix::sys::statvfs
    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        debug!("statfs()");

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
                error!("statfs: failed: {}", e);
                reply.error(Self::nix_err_to_errno(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_fs2_creation() {
        // Create a temp directory for testing
        let temp_dir = std::env::temp_dir().join("passthrough2_test");
        let _ = fs::create_dir_all(&temp_dir);

        let mount_point = std::env::temp_dir().join("passthrough2_mount");
        let _ = fs::create_dir_all(&mount_point);

        let fs = PassthroughFS2::new(&temp_dir, &mount_point);
        assert!(fs.is_ok());

        let fs = fs.unwrap();
        assert_eq!(fs.source_dir(), temp_dir);
        assert_eq!(fs.mount_point(), mount_point);
        assert!(!fs.read_only);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&mount_point);
    }

    #[test]
    fn test_to_real_path() {
        let temp_dir = std::env::temp_dir().join("passthrough2_test_real");
        let _ = fs::create_dir_all(&temp_dir);

        let mount_point = std::env::temp_dir().join("passthrough2_mount_real");

        let fs = PassthroughFS2::new(&temp_dir, &mount_point).unwrap();

        let rel_path = Path::new("subdir/file.txt");
        let expected = temp_dir.join("subdir/file.txt");
        assert_eq!(fs.to_real(rel_path), expected);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_read_only_mode() {
        let temp_dir = std::env::temp_dir().join("passthrough2_test_ro");
        let _ = fs::create_dir_all(&temp_dir);

        let mount_point = std::env::temp_dir().join("passthrough2_mount_ro");

        let fs = PassthroughFS2::new_read_only(&temp_dir, &mount_point).unwrap();
        assert!(fs.read_only);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_fh_allocation() {
        let temp_dir = std::env::temp_dir().join("passthrough2_test_fh");
        let _ = fs::create_dir_all(&temp_dir);

        let mount_point = std::env::temp_dir().join("passthrough2_mount_fh");

        let fs = PassthroughFS2::new(&temp_dir, &mount_point).unwrap();

        // File handles should be allocated sequentially
        let fh1 = fs.alloc_fh();
        let fh2 = fs.alloc_fh();
        let fh3 = fs.alloc_fh();

        assert_eq!(fh1, 1);
        assert_eq!(fh2, 2);
        assert_eq!(fh3, 3);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
