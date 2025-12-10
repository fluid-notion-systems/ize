//! PassthroughFS2 - A simplified passthrough filesystem
//!
//! This implementation differs from the original PassthroughFS in several key ways:
//! 1. Uses **real inodes** from the underlying filesystem instead of synthetic counters
//! 2. Uses **real file descriptors** as FUSE file handles (fh IS the fd)
//! 3. Has no concept of a "database file" - just source_dir ↔ mount_point
//! 4. Implements proper file descriptor lifecycle management via RAII

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{self, EBADF, EINVAL, EIO, ENOENT, ENOTDIR, ENOTEMPTY};
use log::{debug, error, info, warn};

/// TTL for cached attributes (1 second)
const TTL: Duration = Duration::from_secs(1);

/// FUSE root inode number
const FUSE_ROOT_ID: u64 = 1;

/// Stored file handle - keeps the File alive so fd remains valid
struct FileHandle {
    /// The File object that owns the fd - when dropped, fd is automatically closed
    #[allow(dead_code)]
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
/// - Uses real file descriptors as FUSE file handles
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
    /// Maps fd → FileHandle (keeps File alive, fd is also the FUSE fh)
    file_handles: RwLock<HashMap<i32, FileHandle>>,
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
                let fd = fh_val as i32;
                let handles = self.file_handles.read().unwrap();

                if let Some(handle) = handles.get(&fd) {
                    // Check if opened for writing
                    if (handle.flags & (libc::O_WRONLY | libc::O_RDWR)) != 0 {
                        // Can use ftruncate directly on the fd
                        debug!("setattr: using ftruncate on fd {}", fd);
                        let ret = unsafe { libc::ftruncate(fd, new_size as i64) };
                        if ret == 0 {
                            Ok(())
                        } else {
                            Err(io::Error::last_os_error())
                        }
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

        // Handle uid/gid change
        if uid.is_some() || gid.is_some() {
            let new_uid = uid
                .map(|u| u as libc::uid_t)
                .unwrap_or(u32::MAX as libc::uid_t);
            let new_gid = gid
                .map(|g| g as libc::gid_t)
                .unwrap_or(u32::MAX as libc::gid_t);

            let path_cstr = match std::ffi::CString::new(real_path.as_os_str().as_encoded_bytes()) {
                Ok(s) => s,
                Err(_) => {
                    reply.error(EINVAL);
                    return;
                }
            };

            let ret = unsafe {
                libc::chown(
                    path_cstr.as_ptr(),
                    if uid.is_some() {
                        new_uid
                    } else {
                        u32::MAX as libc::uid_t
                    },
                    if gid.is_some() {
                        new_gid
                    } else {
                        u32::MAX as libc::gid_t
                    },
                )
            };

            if ret != 0 {
                let e = io::Error::last_os_error();
                error!("setattr: chown failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
                return;
            }
        }

        // Handle atime/mtime change
        if atime.is_some() || mtime.is_some() {
            let path_cstr = match std::ffi::CString::new(real_path.as_os_str().as_encoded_bytes()) {
                Ok(s) => s,
                Err(_) => {
                    reply.error(EINVAL);
                    return;
                }
            };

            let to_timespec = |t: Option<TimeOrNow>| -> libc::timespec {
                match t {
                    Some(TimeOrNow::SpecificTime(st)) => {
                        let duration = st.duration_since(UNIX_EPOCH).unwrap_or_default();
                        libc::timespec {
                            tv_sec: duration.as_secs() as i64,
                            tv_nsec: duration.subsec_nanos() as i64,
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

            let times = [to_timespec(atime), to_timespec(mtime)];

            let ret =
                unsafe { libc::utimensat(libc::AT_FDCWD, path_cstr.as_ptr(), times.as_ptr(), 0) };

            if ret != 0 {
                let e = io::Error::last_os_error();
                error!("setattr: utimensat failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
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
                // Get the real fd - this IS our file handle
                let fd = file.as_raw_fd();

                // Store File to keep fd alive
                let handle = FileHandle {
                    file,
                    real_path,
                    flags,
                };
                self.file_handles.write().unwrap().insert(fd, handle);

                debug!("open: opened fd {} for inode {}", fd, ino);

                // Return fd as the FUSE file handle
                reply.opened(fd as u64, 0);
            }
            Err(e) => {
                error!("open: failed to open {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
        }
    }

    /// Read data from a file
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

        // fh IS the fd
        let fd = fh as i32;

        // Optional safety check - verify we have this handle
        if !self.file_handles.read().unwrap().contains_key(&fd) {
            warn!("read: fd {} not in file_handles table", fd);
            // Still try to read - the fd might still be valid
        }

        // Use pread for thread-safe positional read
        let mut buf = vec![0u8; size as usize];
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                size as usize,
                offset,
            )
        };

        if n >= 0 {
            buf.truncate(n as usize);
            reply.data(&buf);
        } else {
            let e = io::Error::last_os_error();
            error!("read: pread failed: {}", e);
            reply.error(e.raw_os_error().unwrap_or(EIO));
        }
    }

    /// Write data to a file
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

        // fh IS the fd
        let fd = fh as i32;

        // Optional safety check
        if !self.file_handles.read().unwrap().contains_key(&fd) {
            warn!("write: fd {} not in file_handles table", fd);
        }

        // Use pwrite for thread-safe positional write
        let n =
            unsafe { libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset) };

        if n >= 0 {
            reply.written(n as u32);
        } else {
            let e = io::Error::last_os_error();
            error!("write: pwrite failed: {}", e);
            reply.error(e.raw_os_error().unwrap_or(EIO));
        }
    }

    /// Flush file data
    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        debug!("flush(fh={})", fh);

        let fd = fh as i32;

        // fsync the fd
        if unsafe { libc::fsync(fd) } == 0 {
            reply.ok();
        } else {
            let e = io::Error::last_os_error();
            // EBADF is common if the file was already closed, treat it as success
            if e.raw_os_error() == Some(EBADF) {
                reply.ok();
            } else {
                error!("flush: fsync failed: {}", e);
                reply.error(e.raw_os_error().unwrap_or(EIO));
            }
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

        let fd = fh as i32;

        // Remove from table - File is dropped, fd is automatically closed via RAII
        self.file_handles.write().unwrap().remove(&fd);

        reply.ok();
    }

    /// Synchronize file contents
    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
        debug!("fsync(fh={}, datasync={})", fh, datasync);

        let fd = fh as i32;

        let ret = if datasync {
            unsafe { libc::fdatasync(fd) }
        } else {
            unsafe { libc::fsync(fd) }
        };

        if ret == 0 {
            reply.ok();
        } else {
            let e = io::Error::last_os_error();
            error!("fsync: failed: {}", e);
            reply.error(e.raw_os_error().unwrap_or(EIO));
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
                        let fd = file.as_raw_fd();

                        // Register inode mapping
                        self.register_inode(ino, rel_path);

                        // Store file handle
                        let handle = FileHandle {
                            file,
                            real_path,
                            flags: libc::O_RDWR,
                        };
                        self.file_handles.write().unwrap().insert(fd, handle);

                        let attr = self.metadata_to_attr(&meta, ino);
                        debug!("create: created inode {} with fd {}", ino, fd);
                        reply.created(&TTL, &attr, 0, fd as u64, 0);
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

    /// Check file access permissions
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

        let path_cstr = match std::ffi::CString::new(real_path.as_os_str().as_encoded_bytes()) {
            Ok(s) => s,
            Err(_) => {
                reply.error(EINVAL);
                return;
            }
        };

        let ret = unsafe { libc::access(path_cstr.as_ptr(), mask) };

        if ret == 0 {
            reply.ok();
        } else {
            let e = io::Error::last_os_error();
            reply.error(e.raw_os_error().unwrap_or(EIO));
        }
    }

    /// Get filesystem statistics
    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        debug!("statfs()");

        let path_cstr = match std::ffi::CString::new(self.source_dir.as_os_str().as_encoded_bytes())
        {
            Ok(s) => s,
            Err(_) => {
                reply.error(EINVAL);
                return;
            }
        };

        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(path_cstr.as_ptr(), &mut stat) };

        if ret == 0 {
            reply.statfs(
                stat.f_blocks,
                stat.f_bfree,
                stat.f_bavail,
                stat.f_files,
                stat.f_ffree,
                stat.f_bsize as u32,
                stat.f_namemax as u32,
                stat.f_frsize as u32,
            );
        } else {
            let e = io::Error::last_os_error();
            error!("statfs: failed: {}", e);
            reply.error(e.raw_os_error().unwrap_or(EIO));
        }
    }
}

impl PassthroughFS2 {
    /// Helper to truncate via path (when no valid fh is available)
    fn truncate_via_path(path: &Path, size: u64) -> io::Result<()> {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(size)
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
}
