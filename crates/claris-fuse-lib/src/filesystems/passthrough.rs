use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, Request,
};
use libc::{ENOENT, ENOTDIR, O_APPEND, O_CREAT, O_RDWR, O_WRONLY};
use log::debug;

const TTL: Duration = Duration::from_secs(1); // 1 second

/// A basic passthrough filesystem implementation
pub struct PassthroughFS {
    db_path: PathBuf,
    mount_point: PathBuf,
    read_only: bool,
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
        let fs = Self {
            db_path: db_path.as_ref().to_path_buf(),
            mount_point: mount_point.as_ref().to_path_buf(),
            read_only: false,
        };

        // Check if the database file is inside the mount point
        if fs.is_db_inside_mount_point() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Database file cannot be inside the mount point directory",
            ));
        }

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
    fn real_path(&self, path: &Path) -> PathBuf {
        self.db_source_dir()
            .join(path.strip_prefix("/").unwrap_or(path))
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
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent, name);

        let parent_path = if parent == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", parent))
        };

        let path = parent_path.join(name);
        let real_path = self.real_path(&path);

        match fs::metadata(&real_path) {
            Ok(metadata) => {
                let attr = self.stat_to_fuse_attr(
                    &metadata,
                    path.as_os_str().to_str().unwrap().parse().unwrap_or(2),
                );
                reply.entry(&TTL, &attr, 0);
            }
            Err(err) => {
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino);

        let path = if ino == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", ino))
        };

        let real_path = self.real_path(&path);

        match fs::metadata(&real_path) {
            Ok(metadata) => {
                let attr = self.stat_to_fuse_attr(&metadata, ino);
                reply.attr(&TTL, &attr);
            }
            Err(err) => {
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

        let path = if ino == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", ino))
        };

        let real_path = self.real_path(&path);

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

        let path = if ino == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", ino))
        };

        let real_path = self.real_path(&path);

        if !real_path.is_dir() {
            reply.error(ENOTDIR);
            return;
        }

        let entries = match fs::read_dir(real_path) {
            Ok(entries) => entries,
            Err(err) => {
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
                return;
            }
        };

        let mut entries_vec: Vec<(u64, FileType, OsString)> = vec![];

        // Always add . and .. entries
        entries_vec.push((ino, FileType::Directory, OsString::from(".")));
        entries_vec.push((1, FileType::Directory, OsString::from("..")));

        for (index, entry) in entries.enumerate() {
            match entry {
                Ok(entry) => {
                    let file_name = entry.file_name();

                    // Skip the database file if we're in the root directory
                    if ino == 1 && file_name == self.db_path.file_name().unwrap_or_default() {
                        continue;
                    }

                    let file_path = entry.path();
                    let entry_ino = file_path
                        .to_str()
                        .unwrap_or_default()
                        .parse::<u64>()
                        .unwrap_or_else(|_| (index + 3) as u64);

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

                        entries_vec.push((entry_ino, file_type, file_name));
                    }
                }
                Err(_) => continue,
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

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open(ino={}, flags={})", ino, flags);

        // If filesystem is mounted read-only, reject write operations
        if self.read_only && (flags & (O_WRONLY | O_RDWR | O_APPEND | O_CREAT) != 0) {
            reply.error(libc::EROFS);
            return;
        }

        let path = if ino == 1 {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{}", ino))
        };

        let real_path = self.real_path(&path);

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
                let fd = unsafe { libc::dup(std::os::unix::io::AsRawFd::as_raw_fd(&file)) };
                reply.opened(fd as u64, 0);
            }
            Err(err) => {
                reply.error(err.raw_os_error().unwrap_or(ENOENT));
            }
        }
    }
}
