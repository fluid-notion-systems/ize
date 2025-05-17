use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use fuser::{
    consts::FOPEN_DIRECT_IO, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEntry, ReplyOpen, Request,
};
use libc::{
    mode_t, EACCES, EEXIST, ENOENT, ENOTDIR, O_APPEND, O_CREAT, O_DIRECT, O_RDWR, O_WRONLY,
};
use log::{debug, error, info, warn};

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

        info!("Initialized PassthroughFS with source dir: {:?}", fs.db_source_dir());
        
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
        debug!("Translating virtual path {:?} to real path {:?}", path, result);
        result
    }
    
    // Get an inode number for a path, creating a new one if needed
    fn get_inode_for_path(&mut self, path: &Path) -> u64 {
        let rel_path = if path.starts_with("/") {
            path.strip_prefix("/").unwrap_or(path).to_path_buf()
        } else {
            path.to_path_buf()
        };
        
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

        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", parent))
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

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

        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", parent))
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

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
                        let attr = self.stat_to_fuse_attr(&metadata, 0);
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

        // Get parent path from inode
        let parent_path_buf = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                error!("lookup: parent inode {} not found in map", parent);
                reply.error(ENOENT);
                return;
            }
        };

        // Construct the path to lookup
        let path = if parent_path_buf == Path::new("/") {
            PathBuf::from("/").join(name)
        } else {
            PathBuf::from("/").join(&parent_path_buf).join(name)
        };
        
        debug!("lookup: constructed virtual path {:?}", path);
        
        let real_path = self.real_path(&path);
        debug!("lookup: resolved to real path {:?}", real_path);

        match fs::metadata(&real_path) {
            Ok(metadata) => {
                // Get or create inode for this path
                let ino = self.get_inode_for_path(&path);
                debug!("lookup: assigned inode {} to path {:?}", ino, path);
                
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
                    error!("getattr: error accessing root directory {:?}: {}", real_path, err);
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
                    let entry_path = if dir_path == Path::new("/") || dir_path.as_os_str().is_empty() {
                        PathBuf::from("/").join(&file_name)
                    } else {
                        PathBuf::from("/").join(&dir_path).join(&file_name)
                    };
                    
                    debug!("readdir: processing entry {:?}, virtual path {:?}", file_name, entry_path);

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
                        debug!("readdir: assigned inode {} to path {:?}", entry_ino, entry_path);
                        
                        entries_vec.push((entry_ino, file_type, file_name));
                    }
                }
                Err(e) => {
                    warn!("readdir: error processing directory entry: {}", e);
                    continue;
                },
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
            reply.error(libc::EROFS);
            return;
        }

        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", parent))
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

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
                let _ = file.set_permissions(permissions);

                // Get file attributes
                match fs::metadata(&real_path) {
                    Ok(metadata) => {
                        let attr = self.stat_to_fuse_attr(&metadata, 0);
                        // Use file descriptor as file handle
                        let fd = unsafe { libc::dup(file.as_raw_fd()) };
                        reply.created(&TTL, &attr, 0, fd as u64, 0);
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
