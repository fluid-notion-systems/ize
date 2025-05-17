use std::collections::HashMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    consts::FOPEN_DIRECT_IO, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{
    mode_t, timespec, EACCES, EEXIST, ENOENT, ENOTDIR, O_APPEND, O_CREAT, O_DIRECT, O_RDWR,
    O_WRONLY, UTIME_NOW, UTIME_OMIT,
};
use log::{debug, error, info, warn};

use super::error::{FsError, FsErrorCode, IoErrorExt};

const TTL: Duration = Duration::from_secs(1); // 1 second

/// A basic passthrough filesystem implementation
pub struct PassthroughFS {
    db_path: PathBuf,
    mount_point: PathBuf,
    read_only: bool,
    // Map from inode numbers to paths (relative to source dir)
    inode_map: HashMap<u64, PathBuf>,
    // Map from paths to inode numbers
    path_map: HashMap<PathBuf, u64>,
    // Counter for generating new inode numbers
    next_inode: AtomicU64,
}

impl PassthroughFS {
    /// Create a new passthrough filesystem
    ///
    /// # Errors
    /// Returns an error if the database file is within the mount point directory
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(
        db_path: P,
        mount_point: Q,
    ) -> std::io::Result<Self> {
        let mut fs = Self {
            db_path: db_path.as_ref().to_path_buf(),
            mount_point: mount_point.as_ref().to_path_buf(),
            read_only: false,
            inode_map: HashMap::new(),
            path_map: HashMap::new(),
            next_inode: AtomicU64::new(2), // Start from 2, 1 is reserved for root
        };

        // Check if the database file is inside the mount point
        if fs.is_db_inside_mount_point() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Database file cannot be inside the mount point directory",
            ));
        }

        // Initialize the root directory with inode 1
        fs.inode_map.insert(1, PathBuf::from(""));
        fs.path_map.insert(PathBuf::from(""), 1);

        info!(
            "Initialized PassthroughFS with source dir: {:?}",
            fs.db_source_dir()
        );

        Ok(fs)
    }

    /// Check if the database file is inside the mount point directory
    fn is_db_inside_mount_point(&self) -> bool {
        let db_path_canon = match std::fs::canonicalize(&self.db_path) {
            Ok(path) => path,
            Err(_) => return false, // Can't determine, assume it's not inside
        };

        let mount_point_canon = match std::fs::canonicalize(&self.mount_point) {
            Ok(path) => path,
            Err(_) => return false, // Can't determine, assume it's not inside
        };

        db_path_canon.starts_with(mount_point_canon)
    }

    /// Get the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Get the mount point
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    /// Get the source directory (parent directory of the database file)
    fn db_source_dir(&self) -> PathBuf {
        self.db_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    /// Create a new passthrough filesystem with read-only mode
    ///
    /// # Errors
    /// Returns an error if the database file is within the mount point directory
    pub fn new_read_only<P: AsRef<Path>, Q: AsRef<Path>>(
        db_path: P,
        mount_point: Q,
    ) -> std::io::Result<Self> {
        let mut fs = Self::new(db_path, mount_point)?;
        fs.read_only = true;
        info!("Setting filesystem to read-only mode");
        Ok(fs)
    }

    /// Set read-only mode
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Mount the filesystem
    pub fn mount(self) -> std::io::Result<()> {
        let mut options = vec![MountOption::FSName("claris-fuse".to_string())];

        if self.read_only {
            options.push(MountOption::RO);
        }

        // Check if database file exists
        if !self.db_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Database file not found: {:?}", self.db_path),
            ));
        }

        // We need to clone mount_point because fuser::mount2 takes ownership of self
        let mount_point = self.mount_point.clone();
        fuser::mount2(self, mount_point, &options)?;
        Ok(())
    }

    // Helper method to get the real path on the underlying filesystem
    pub fn real_path(&self, path: &Path) -> PathBuf {
        let clean_path = path.strip_prefix("/").unwrap_or(path);
        let result = self.db_source_dir().join(clean_path);
        debug!(
            "Translating virtual path {:?} to real path {:?}",
            path, result
        );
        result
    }

    // Get an inode number for a path, creating a new one if needed
    fn get_inode_for_path(&mut self, path: &Path) -> u64 {
        // Ensure we're using a relative path (no leading slash)
        let rel_path = path.strip_prefix("/").unwrap_or(path).to_path_buf();

        if let Some(&ino) = self.path_map.get(&rel_path) {
            return ino;
        }

        let ino = self.next_inode.fetch_add(1, Ordering::SeqCst);
        self.path_map.insert(rel_path.clone(), ino);
        self.inode_map.insert(ino, rel_path);
        ino
    }

    // Get a path for an inode number
    fn get_path_for_inode(&self, ino: u64) -> Option<PathBuf> {
        if ino == 1 {
            return Some(PathBuf::from("/"));
        }
        self.inode_map.get(&ino).cloned()
    }

    // Helper to convert a file's metadata to FUSE file attributes
    fn stat_to_fuse_attr(&self, stat: &fs::Metadata, ino: u64) -> FileAttr {
        let kind = if stat.is_dir() {
            FileType::Directory
        } else if stat.is_file() {
            FileType::RegularFile
        } else if stat.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::RegularFile
        };

        FileAttr {
            ino,
            size: stat.size(),
            blocks: stat.blocks(),
            atime: SystemTime::UNIX_EPOCH + Duration::from_secs(stat.atime() as u64),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(stat.mtime() as u64),
            ctime: SystemTime::UNIX_EPOCH + Duration::from_secs(stat.ctime() as u64),
            crtime: SystemTime::UNIX_EPOCH,
            kind,
            perm: stat.mode() as u16 & 0o7777,
            nlink: stat.nlink() as u32,
            uid: stat.uid(),
            gid: stat.gid(),
            rdev: stat.rdev() as u32,
            flags: 0,
            blksize: stat.blksize() as u32,
        }
    }
}

impl Filesystem for PassthroughFS {
    fn setattr(
        &mut self,
        _req: &Request,
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

        // Get path from inode
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("setattr: inode {} not found in map", ino);
                reply.error(ENOENT);
                return;
            }
        };

        let real_path = self.real_path(&path);

        // Update file attributes based on provided options
        match fs::metadata(&real_path) {
            Ok(metadata) => {
                let mut changed = false;

                // Handle file size change (truncate)
                if let Some(size) = size {
                    debug!("setattr: truncating file {:?} to size {}", real_path, size);

                    // If we have a file handle already, use it
                    if let Some(fh) = fh {
                        let fd = fh as i32;
                        let result = unsafe { libc::ftruncate(fd, size as i64) };

                        if result != 0 {
                            let err = std::io::Error::last_os_error();
                            error!("setattr: failed to ftruncate file: {}", err);
                            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                            return;
                        }
                    } else {
                        // Otherwise open the file
                        match fs::OpenOptions::new().write(true).open(&real_path) {
                            Ok(file) => {
                                if let Err(e) = file.set_len(size) {
                                    error!("setattr: failed to set file size: {}", e);
                                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                                    return;
                                }
                            }
                            Err(e) => {
                                error!("setattr: failed to open file for truncate: {}", e);
                                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                                return;
                            }
                        }
                    }
                    changed = true;
                }

                // Handle permissions change
                if let Some(mode) = mode {
                    debug!("setattr: changing mode of {:?} to {:o}", real_path, mode);
                    let permissions = fs::Permissions::from_mode(mode & 0o777);
                    if let Err(e) = fs::set_permissions(&real_path, permissions) {
                        error!("setattr: failed to set permissions: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }
                    changed = true;
                }

                // Handle ownership change
                if uid.is_some() || gid.is_some() {
                    error!("setattr: uid/gid change not implemented");
                    reply.error(libc::ENOSYS);
                    return;
                }

                // Handle timestamp changes
                if atime.is_some() || mtime.is_some() {
                    debug!("setattr: setting timestamps for {:?}", real_path);

                    // Convert path to C string
                    let path_cstr = match CString::new(real_path.to_str().unwrap_or("")) {
                        Ok(s) => s,
                        Err(_) => {
                            error!("setattr: path contains null bytes");
                            reply.error(libc::EINVAL);
                            return;
                        }
                    };

                    // Prepare timespec structs for atime and mtime
                    let mut times: [timespec; 2] = [
                        timespec {
                            tv_sec: 0,
                            tv_nsec: UTIME_OMIT,
                        }, // atime - omit by default
                        timespec {
                            tv_sec: 0,
                            tv_nsec: UTIME_OMIT,
                        }, // mtime - omit by default
                    ];

                    // Set atime if provided
                    if let Some(time) = atime {
                        match time {
                            TimeOrNow::SpecificTime(t) => {
                                let duration = t.duration_since(UNIX_EPOCH).unwrap_or_default();
                                times[0].tv_sec = duration.as_secs() as i64;
                                times[0].tv_nsec = duration.subsec_nanos() as i64;
                            }
                            TimeOrNow::Now => {
                                times[0].tv_nsec = UTIME_NOW;
                            }
                        }
                    }

                    // Set mtime if provided
                    if let Some(time) = mtime {
                        match time {
                            TimeOrNow::SpecificTime(t) => {
                                let duration = t.duration_since(UNIX_EPOCH).unwrap_or_default();
                                times[1].tv_sec = duration.as_secs() as i64;
                                times[1].tv_nsec = duration.subsec_nanos() as i64;
                            }
                            TimeOrNow::Now => {
                                times[1].tv_nsec = UTIME_NOW;
                            }
                        }
                    }

                    // Call utimensat to update the timestamps
                    let res = unsafe {
                        libc::utimensat(
                            libc::AT_FDCWD,
                            path_cstr.as_ptr(),
                            times.as_ptr(),
                            0, // No flags
                        )
                    };

                    if res != 0 {
                        let err = std::io::Error::last_os_error();
                        error!("setattr: failed to set timestamps: {}", err);
                        reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }

                    changed = true;
                }

                // Get updated attributes and reply
                if changed {
                    match fs::metadata(&real_path) {
                        Ok(updated_metadata) => {
                            let attr = self.stat_to_fuse_attr(&updated_metadata, ino);
                            reply.attr(&TTL, &attr);
                        }
                        Err(e) => {
                            error!("setattr: failed to get updated metadata: {}", e);
                            reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        }
                    }
                } else {
                    // Nothing changed, return current attributes
                    let attr = self.stat_to_fuse_attr(&metadata, ino);
                    reply.attr(&TTL, &attr);
                }
            }
            Err(e) => {
                error!("setattr: failed to get metadata for {:?}: {}", real_path, e);
                reply.error(e.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }
    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent, name);

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(FsError::ReadOnlyFs.to_error_code());
            return;
        }

        // Construct path based on parent inode
        let path = if parent == 1 {
            PathBuf::from("/").join(name)
        } else if let Some(parent_rel_path) = self.inode_map.get(&parent) {
            PathBuf::from("/").join(parent_rel_path).join(name)
        } else {
            error!("unlink: parent inode {} not found in inode map", parent);
            reply.error(libc::ENOENT);
            return;
        };
        let real_path = self.real_path(&path);

        debug!("unlink: removing file at real path: {:?}", real_path);

        // Remove the file
        match fs::remove_file(&real_path) {
            Ok(_) => {
                // Remove from inode mapping if applicable
                let rel_path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
                // Find the inode for this path first, then remove it
                let inode_to_remove = if let Some(&ino) = self.path_map.get(&rel_path) {
                    Some(ino)
                } else {
                    None
                };

                if let Some(ino) = inode_to_remove {
                    self.inode_map.remove(&ino);
                    self.path_map.remove(&rel_path);
                    debug!("unlink: removed inode {} for path {:?}", ino, rel_path);
                }

                reply.ok();
            }
            Err(e) => {
                let fs_error = e.into_fs_error(&real_path);
                error!("unlink: error: {}", fs_error);
                reply.error(fs_error.to_error_code());
            }
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        debug!(
            "write(ino={}, fh={}, offset={}, data.len()={}, flags=0x{:x})",
            ino,
            fh,
            offset,
            data.len(),
            _flags
        );

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(FsError::ReadOnlyFs.to_error_code());
            return;
        }

        // Get the path for this inode
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("write: inode {} not found in map", ino);
                reply.error(libc::ENOENT);
                return;
            }
        };

        let real_path = self.real_path(&path);

        // Use Rust's File API instead of raw file descriptors
        // This avoids issues with stale file handles after operations like truncate
        match fs::OpenOptions::new().write(true).open(&real_path) {
            Ok(mut file) => {
                // Seek to the correct position
                if let Err(e) = file.seek(SeekFrom::Start(offset as u64)) {
                    error!("write: failed to seek: {}", e);
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }

                // Write the data
                match file.write(data) {
                    Ok(bytes_written) => {
                        debug!("write: successfully wrote {} bytes", bytes_written);
                        reply.written(bytes_written as u32);
                    }
                    Err(e) => {
                        error!("write: failed to write: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    }
                }
            }
            Err(e) => {
                error!("write: failed to open file: {}", e);
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn flush(&mut self, _req: &Request, ino: u64, fh: u64, lock_owner: u64, reply: ReplyEmpty) {
        debug!("flush(ino={}, fh={}, lock_owner={})", ino, fh, lock_owner);

        // In a passthrough filesystem, we can simply pass the flush operation to the OS
        // by calling fsync on the file descriptor
        let fd = fh as i32;
        let result = unsafe { libc::fsync(fd) };

        if result == 0 {
            reply.ok();
        } else {
            let err = std::io::Error::last_os_error();
            error!("flush error: {}", err);
            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent, name);

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(FsError::ReadOnlyFs.to_error_code());
            return;
        }

        // Construct path based on parent inode
        let path = if parent == 1 {
            PathBuf::from("/").join(name)
        } else if let Some(parent_rel_path) = self.inode_map.get(&parent) {
            PathBuf::from("/").join(parent_rel_path).join(name)
        } else {
            error!("rmdir: parent inode {} not found in inode map", parent);
            reply.error(libc::ENOENT);
            return;
        };
        let real_path = self.real_path(&path);

        debug!("rmdir: removing directory at real path: {:?}", real_path);

        // Remove the directory
        match fs::remove_dir(&real_path) {
            Ok(_) => {
                // Remove from inode mapping if applicable
                let rel_path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
                // Find the inode for this path first, then remove it
                let inode_to_remove = if let Some(&ino) = self.path_map.get(&rel_path) {
                    Some(ino)
                } else {
                    None
                };

                if let Some(ino) = inode_to_remove {
                    self.inode_map.remove(&ino);
                    self.path_map.remove(&rel_path);
                    debug!("rmdir: removed inode {} for path {:?}", ino, rel_path);
                }

                reply.ok();
            }
            Err(e) => {
                let fs_error = e.into_fs_error(&real_path);
                error!("rmdir: error: {}", fs_error);
                reply.error(fs_error.to_error_code());
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        flags: i32,
        _lock_owner: Option<u64>,
        flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!(
            "release(ino={}, fh={}, flags=0x{:x}, flush={})",
            ino, fh, flags, flush
        );

        // Close the file descriptor
        let fd = fh as i32;
        let result = unsafe { libc::close(fd) };

        if result == 0 {
            reply.ok();
        } else {
            let err = std::io::Error::last_os_error();
            error!("release error: {}", err);
            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
        }
    }

    fn mknod(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        rdev: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        debug!(
            "mknod(parent={}, name={:?}, mode=0{:o}, rdev={}, umask=0{:o})",
            parent, name, mode, rdev, umask
        );

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get the parent path from the inode map
        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => PathBuf::from("/").join(p),
                None => {
                    error!("create: parent inode {} not found in map", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

        debug!("Creating file at real path: {:?}", real_path);
        // Check if the file already exists
        if real_path.exists() {
            reply.error(EEXIST);
            return;
        }

        // Apply umask to mode
        #[allow(clippy::unnecessary_cast)]
        let mode_with_umask = mode & !(umask as u32);

        // Create the file
        let res = if (mode & libc::S_IFREG) != 0 {
            // Regular file
            match File::create(&real_path) {
                Ok(file) => {
                    // Set the permissions
                    let permissions = fs::Permissions::from_mode(mode_with_umask & 0o777);
                    let _ = file.set_permissions(permissions);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else if (mode & libc::S_IFIFO) != 0 {
            // FIFO (named pipe)
            nix::unistd::mkfifo(
                &real_path,
                nix::sys::stat::Mode::from_bits_truncate(mode_with_umask & 0o777),
            )
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        } else if (mode & libc::S_IFCHR) != 0 || (mode & libc::S_IFBLK) != 0 {
            // Character or block device
            // This requires root privileges and is typically not needed
            #[cfg(target_os = "linux")]
            {
                use nix::sys::stat::{makedev, SFlag};

                let file_type = if (mode & libc::S_IFCHR) != 0 {
                    SFlag::S_IFCHR
                } else {
                    SFlag::S_IFBLK
                };

                // Combine file type with permissions
                let mode_bits = (file_type.bits() | (mode_with_umask & 0o777)) as mode_t;
                let dev = makedev((rdev >> 8) as u64, (rdev & 0xff) as u64);

                // Use the nix system call
                unsafe {
                    let ret = libc::mknod(
                        std::ffi::CString::new(real_path.to_str().unwrap())
                            .unwrap()
                            .as_ptr(),
                        mode_bits,
                        dev as libc::dev_t,
                    );

                    if ret == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            }

            #[cfg(not(target_os = "linux"))]
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Device creation not supported on this platform",
                ))
            }
        } else {
            // Unsupported file type
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Unsupported file type",
            ))
        };

        match res {
            Ok(_) => match fs::metadata(&real_path) {
                Ok(metadata) => {
                    let attr = self.stat_to_fuse_attr(&metadata, 0);
                    reply.entry(&TTL, &attr, 0);
                }
                Err(e) => {
                    reply.error(e.raw_os_error().unwrap_or(ENOENT));
                }
            },
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(EACCES));
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        debug!(
            "mkdir(parent={}, name={:?}, mode=0{:o}, umask=0{:o})",
            parent, name, mode, umask
        );

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(libc::EROFS);
            return;
        }

        // Get the parent path from the inode map
        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            match self.get_path_for_inode(parent) {
                Some(p) => PathBuf::from("/").join(p),
                None => {
                    error!("mkdir: parent inode {} not found in map", parent);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

        debug!("mkdir: creating directory at real path: {:?}", real_path);

        // Apply umask to mode
        #[allow(clippy::unnecessary_cast)]
        let mode_with_umask = mode & !(umask as u32);

        // Create the directory with specified permissions
        match fs::create_dir(&real_path) {
            Ok(_) => {
                // Set permissions
                if let Ok(metadata) = fs::metadata(&real_path) {
                    let mut permissions = metadata.permissions();
                    permissions.set_mode(mode_with_umask & 0o777);
                    let _ = fs::set_permissions(&real_path, permissions);
                }

                // Return directory attributes
                match fs::metadata(&real_path) {
                    Ok(metadata) => {
                        // Generate a new inode number for this directory
                        let rel_path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
                        let ino = self.get_inode_for_path(&rel_path);
                        debug!("mkdir: assigned inode {} to path {:?}", ino, rel_path);

                        let attr = self.stat_to_fuse_attr(&metadata, ino);
                        reply.entry(&TTL, &attr, 0);
                    }
                    Err(e) => {
                        reply.error(e.raw_os_error().unwrap_or(ENOENT));
                    }
                }
            }
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(EACCES));
            }
        }
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent, name);

        // Special case for root directory
        if parent == 1 && !self.path_map.contains_key(&PathBuf::from("")) {
            // Make sure root is in our maps
            self.inode_map.insert(1, PathBuf::from(""));
            self.path_map.insert(PathBuf::from(""), 1);
        }

        // Get parent path from inode
        let parent_path_buf = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                error!("lookup: parent inode {} not found in map", parent);
                reply.error(ENOENT);
                return;
            }
        };

        // Construct the path to lookup - make sure all paths are treated consistently
        let path = if parent == 1 || parent_path_buf.as_os_str().is_empty() {
            // Root directory
            PathBuf::from("/").join(name)
        } else {
            // Non-root directory
            PathBuf::from("/").join(&parent_path_buf).join(name)
        };

        debug!("lookup: constructed virtual path {:?}", path);

        let real_path = self.real_path(&path);
        debug!("lookup: resolved to real path {:?}", real_path);

        match fs::metadata(&real_path) {
            Ok(metadata) => {
                // Get or create inode for this path
                let rel_path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
                let ino = self.get_inode_for_path(&rel_path);
                debug!("lookup: assigned inode {} to path {:?}", ino, rel_path);

                let attr = self.stat_to_fuse_attr(&metadata, ino);
                reply.entry(&TTL, &attr, 0);
            }
            Err(err) => {
                error!("lookup: error accessing {:?}: {}", real_path, err);
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino);

        // Special case for root
        if ino == 1 {
            let real_path = self.db_source_dir();
            match fs::metadata(&real_path) {
                Ok(metadata) => {
                    let attr = self.stat_to_fuse_attr(&metadata, 1);
                    reply.attr(&TTL, &attr);
                }
                Err(err) => {
                    error!(
                        "getattr: error accessing root directory {:?}: {}",
                        real_path, err
                    );
                    reply.error(err.raw_os_error().unwrap_or(ENOENT));
                }
            }
            return;
        }

        // Get path from inode
        let rel_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("getattr: inode {} not found in map", ino);
                reply.error(ENOENT);
                return;
            }
        };

        debug!("getattr: found path {:?} for inode {}", rel_path, ino);

        let real_path = self.real_path(&rel_path);
        debug!("getattr: resolved to real path {:?}", real_path);

        match fs::metadata(&real_path) {
            Ok(metadata) => {
                let attr = self.stat_to_fuse_attr(&metadata, ino);
                reply.attr(&TTL, &attr);
            }
            Err(err) => {
                error!("getattr: error accessing {:?}: {}", real_path, err);
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        debug!("read(ino={}, offset={}, size={})", ino, offset, size);

        // Get path from inode
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("read: inode {} not found in map", ino);
                reply.error(ENOENT);
                return;
            }
        };

        debug!("read: found path {:?} for inode {}", path, ino);

        let real_path = self.real_path(&path);
        debug!("read: resolved to real path {:?}", real_path);

        match File::open(&real_path) {
            Ok(mut file) => {
                let mut buffer = vec![0; size as usize];

                match file.seek(SeekFrom::Start(offset as u64)) {
                    Ok(_) => match file.read(&mut buffer) {
                        Ok(n) => {
                            buffer.truncate(n);
                            reply.data(&buffer);
                        }
                        Err(err) => {
                            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                        }
                    },
                    Err(err) => {
                        reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                    }
                }
            }
            Err(err) => {
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir(ino={}, offset={})", ino, offset);

        // Get path from inode
        let dir_path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("readdir: inode {} not found in map", ino);
                reply.error(ENOENT);
                return;
            }
        };

        debug!("readdir: found path {:?} for inode {}", dir_path, ino);

        let real_path = self.real_path(&dir_path);
        debug!("readdir: resolved to real path {:?}", real_path);

        if !real_path.is_dir() {
            error!("readdir: path {:?} is not a directory", real_path);
            reply.error(ENOTDIR);
            return;
        }

        let entries = match fs::read_dir(&real_path) {
            Ok(entries) => entries,
            Err(err) => {
                error!("readdir: error reading directory {:?}: {}", real_path, err);
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
                return;
            }
        };

        let mut entries_vec: Vec<(u64, FileType, OsString)> = vec![];

        // Always add . and .. entries
        entries_vec.push((ino, FileType::Directory, OsString::from(".")));

        // For '..' use the parent's inode, or 1 for the root
        let parent_ino = if ino == 1 {
            1 // Root's parent is root
        } else {
            // Get parent path
            let parent_path = dir_path.parent().unwrap_or(Path::new(""));
            // Get inode for parent path
            self.get_inode_for_path(parent_path)
        };
        entries_vec.push((parent_ino, FileType::Directory, OsString::from("..")));

        for entry in entries {
            match entry {
                Ok(entry) => {
                    let file_name = entry.file_name();

                    // Skip the database file if we're in the root directory
                    if ino == 1 && file_name == self.db_path.file_name().unwrap_or_default() {
                        debug!("readdir: skipping database file {:?}", file_name);
                        continue;
                    }

                    // Construct virtual path for this entry
                    let entry_path =
                        if dir_path == Path::new("/") || dir_path.as_os_str().is_empty() {
                            PathBuf::from("/").join(&file_name)
                        } else {
                            PathBuf::from("/").join(&dir_path).join(&file_name)
                        };

                    debug!(
                        "readdir: processing entry {:?}, virtual path {:?}",
                        file_name, entry_path
                    );

                    if let Ok(metadata) = entry.metadata() {
                        let file_type = if metadata.is_dir() {
                            FileType::Directory
                        } else if metadata.is_file() {
                            FileType::RegularFile
                        } else if metadata.file_type().is_symlink() {
                            FileType::Symlink
                        } else {
                            FileType::RegularFile
                        };

                        // Get or create inode for this path
                        let entry_ino = self.get_inode_for_path(&entry_path);
                        debug!(
                            "readdir: assigned inode {} to path {:?}",
                            entry_ino, entry_path
                        );

                        entries_vec.push((entry_ino, file_type, file_name));
                    }
                }
                Err(e) => {
                    warn!("readdir: error processing directory entry: {}", e);
                    continue;
                }
            }
        }

        for (i, entry) in entries_vec.into_iter().enumerate().skip(offset as usize) {
            let (ino, file_type, name) = entry;
            let full = reply.add(ino, (i + 1) as i64, file_type, name);
            if full {
                break;
            }
        }

        reply.ok();
    }

    fn rename(
        &mut self,
        _req: &Request,
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

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(FsError::ReadOnlyFs.to_error_code());
            return;
        }

        // Construct source path based on parent inode
        let src_path = if parent == 1 {
            PathBuf::from("/").join(name)
        } else if let Some(parent_rel_path) = self.inode_map.get(&parent) {
            PathBuf::from("/").join(parent_rel_path).join(name)
        } else {
            error!(
                "rename: source parent inode {} not found in inode map",
                parent
            );
            reply.error(libc::ENOENT);
            return;
        };

        // Construct destination path based on new parent inode
        let dst_path = if newparent == 1 {
            PathBuf::from("/").join(newname)
        } else if let Some(newparent_rel_path) = self.inode_map.get(&newparent) {
            PathBuf::from("/").join(newparent_rel_path).join(newname)
        } else {
            error!(
                "rename: destination parent inode {} not found in inode map",
                newparent
            );
            reply.error(libc::ENOENT);
            return;
        };

        let real_src_path = self.real_path(&src_path);
        let real_dst_path = self.real_path(&dst_path);

        debug!("rename: from {:?} to {:?}", real_src_path, real_dst_path);

        // Ensure parent directory of destination exists
        if let Some(parent_dir) = real_dst_path.parent() {
            if !parent_dir.exists() {
                if let Err(e) = fs::create_dir_all(parent_dir) {
                    let fs_error = e.into_fs_error(parent_dir);
                    error!("rename: failed to create parent directory: {}", fs_error);
                    reply.error(fs_error.to_error_code());
                    return;
                }
            }
        }

        // Perform the rename operation
        match fs::rename(&real_src_path, &real_dst_path) {
            Ok(_) => {
                // Update the inode mapping if this path has an associated inode
                let src_rel_path = src_path
                    .strip_prefix("/")
                    .unwrap_or(&src_path)
                    .to_path_buf();
                let dst_rel_path = dst_path
                    .strip_prefix("/")
                    .unwrap_or(&dst_path)
                    .to_path_buf();

                // Find the inode for this path
                let inode_to_update = if let Some(&ino) = self.path_map.get(&src_rel_path) {
                    Some(ino)
                } else {
                    None
                };

                // Update the mappings if found
                if let Some(ino) = inode_to_update {
                    self.inode_map.remove(&ino);
                    self.path_map.remove(&src_rel_path);

                    self.inode_map.insert(ino, dst_rel_path.clone());
                    self.path_map.insert(dst_rel_path.clone(), ino);

                    debug!(
                        "rename: updated inode {} from {:?} to {:?}",
                        ino, src_rel_path, dst_rel_path
                    );
                }

                reply.ok();
            }
            Err(e) => {
                let fs_error = e.into_fs_error(&real_src_path);
                error!("rename: error: {}", fs_error);
                reply.error(fs_error.to_error_code());
            }
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        flags: u32,
        umask: i32,
        reply: ReplyCreate,
    ) {
        debug!(
            "create(parent={}, name={:?}, mode=0{:o}, flags={}, umask=0{:o})",
            parent, name, mode, flags, umask as u32
        );

        // Check if filesystem is mounted read-only
        if self.read_only {
            reply.error(FsError::ReadOnlyFs.to_error_code());
            return;
        }

        // Get the parent path from the inode map
        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else if let Some(parent_rel_path) = self.inode_map.get(&parent) {
            PathBuf::from("/").join(parent_rel_path)
        } else {
            error!("create: parent inode {} not found in inode map", parent);
            reply.error(libc::ENOENT);
            return;
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

        debug!("Creating file at real path: {:?}", real_path);

        // Ensure parent directory exists
        if let Some(parent_dir) = real_path.parent() {
            if !parent_dir.exists() {
                debug!(
                    "Parent directory does not exist, creating: {:?}",
                    parent_dir
                );
                if let Err(e) = fs::create_dir_all(parent_dir) {
                    let fs_error = e.into_fs_error(parent_dir);
                    error!("Failed to create parent directory: {}", fs_error);
                    reply.error(fs_error.to_error_code());
                    return;
                }
            }
        }

        // Apply umask to mode
        #[allow(clippy::unnecessary_cast)]
        let mode_with_umask = mode & !(umask as u32);

        // Open/Create file with proper flags and mode
        let mut options = fs::OpenOptions::new();
        options.create(true).write(true);

        if (flags as i32) & O_APPEND != 0 {
            options.append(true);
        }

        match (flags as i32) & O_RDWR {
            O_RDWR => {
                options.read(true);
            }
            O_WRONLY => {} // Write already set
            _ => {
                options.read(true);
            }
        }

        match options.open(&real_path) {
            Ok(file) => {
                // Set the file permissions
                let permissions = fs::Permissions::from_mode(mode_with_umask & 0o777);
                if let Err(e) = file.set_permissions(permissions) {
                    error!("Failed to set file permissions: {}", e);
                }

                // Generate a new inode number for the file
                let rel_path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
                let inode = self.get_inode_for_path(&rel_path);

                debug!("Assigned inode {} to path {:?}", inode, rel_path);

                // Get file attributes
                match fs::metadata(&real_path) {
                    Ok(metadata) => {
                        let attr = self.stat_to_fuse_attr(&metadata, inode);

                        // Use file descriptor as file handle
                        let fd = unsafe { libc::dup(file.as_raw_fd()) };
                        if fd < 0 {
                            let err = std::io::Error::last_os_error();
                            error!("Failed to duplicate file descriptor: {}", err);
                            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                            return;
                        }

                        debug!(
                            "File created successfully at {:?} with inode {}",
                            real_path, inode
                        );
                        reply.created(&TTL, &attr, 0, fd as u64, 0);
                    }
                    Err(e) => {
                        let fs_error = e.into_fs_error(&real_path);
                        error!("Failed to get file metadata: {}", fs_error);
                        reply.error(fs_error.to_error_code());
                    }
                }
            }
            Err(e) => {
                let fs_error = e.into_fs_error(&real_path);
                error!("Failed to create file: {}", fs_error);
                reply.error(fs_error.to_error_code());
            }
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open(ino={}, flags={})", ino, flags);

        // If filesystem is mounted read-only, reject write operations
        if self.read_only && (flags & (O_WRONLY | O_RDWR | O_APPEND | O_CREAT) != 0) {
            error!("open: attempting write operation on read-only filesystem");
            reply.error(libc::EROFS);
            return;
        }

        // Get path from inode
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("open: inode {} not found in map", ino);
                reply.error(ENOENT);
                return;
            }
        };

        debug!("open: found path {:?} for inode {}", path, ino);

        let real_path = self.real_path(&path);
        debug!("open: resolved to real path {:?}", real_path);

        let mut options = fs::OpenOptions::new();

        if flags & O_APPEND != 0 {
            options.append(true);
        }

        if flags & O_CREAT != 0 {
            options.create(true);
        }

        match flags & O_RDWR {
            O_RDWR => {
                options.read(true).write(true);
            }
            O_WRONLY => {
                options.write(true);
            }
            _ => {
                options.read(true);
            }
        }

        match options.open(&real_path) {
            Ok(file) => {
                // Use file descriptor as file handle
                let fd = unsafe { libc::dup(file.as_raw_fd()) };

                // Set direct_io flag for better performance with some applications
                #[allow(clippy::unnecessary_cast)]
                let direct_io = (flags & (O_DIRECT as i32)) != 0;
                let _keep_cache = !direct_io;

                reply.opened(fd as u64, if direct_io { FOPEN_DIRECT_IO } else { 0 });
            }
            Err(err) => {
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }
}
