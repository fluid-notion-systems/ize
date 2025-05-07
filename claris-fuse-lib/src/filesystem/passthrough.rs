use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyOpen, Request,
};
use libc::{ENOENT, ENOTDIR, O_APPEND, O_CREAT, O_RDWR, O_WRONLY};
use log::debug;

const TTL: Duration = Duration::from_secs(1); // 1 second

/// A basic passthrough filesystem implementation
pub struct PassthroughFS {
    source_path: PathBuf,
}

impl PassthroughFS {
    /// Create a new passthrough filesystem
    pub fn new<P: AsRef<Path>>(source_path: P) -> Self {
        Self {
            source_path: source_path.as_ref().to_path_buf(),
        }
    }

    /// Get the source path 
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Mount the filesystem at the given path
    pub fn mount<P: AsRef<Path>>(self, mountpoint: P) -> std::io::Result<()> {
        let options = vec![MountOption::RO, MountOption::FSName("claris-fuse".to_string())];
        fuser::mount2(self, mountpoint, &options)?;
        Ok(())
    }

    // Helper method to get the real path on the underlying filesystem
    fn real_path(&self, path: &Path) -> PathBuf {
        self.source_path.join(path.strip_prefix("/").unwrap_or(path))
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
                let attr = self.stat_to_fuse_attr(&metadata, path.as_os_str().to_str().unwrap().parse().unwrap_or(2));
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
                    let file_path = entry.path();
                    let entry_ino = file_path.to_str()
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