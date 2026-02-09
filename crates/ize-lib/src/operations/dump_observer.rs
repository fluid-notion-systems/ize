//! Dump observer for `--dump` mode.
//!
//! `DumpObserver` implements [`FsObserver`] and writes human-readable
//! filesystem operation logs directly to a file. Unlike [`OpcodeRecorder`],
//! it does **not** use an [`OpcodeQueue`] or consumer thread — it writes
//! synchronously through a [`BufWriter`] behind a [`Mutex`].
//!
//! This is intended purely for debugging / inspection and must never
//! interfere with the production opcode pipeline.
//!
//! # Example
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use std::path::PathBuf;
//! # use ize_lib::filesystems::passthrough_fd::FdPassthroughFS;
//! # use ize_lib::filesystems::observing::ObservingFS;
//! # use ize_lib::operations::DumpObserver;
//! # fn main() -> std::io::Result<()> {
//! # let source_dir = PathBuf::from("/tmp/source");
//! # let inode_map = todo!();
//! let dump_path = std::env::temp_dir().join("ize-dump.log");
//! let dump = DumpObserver::open(inode_map, source_dir, &dump_path)?;
//! // optionally: dump.with_ignore_filters(filters);
//!
//! // let mut observing = ObservingFS::new(fs);
//! // observing.add_observer(Arc::new(dump));
//! # Ok(())
//! # }
//! ```

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{debug, warn};

use crate::filesystems::observing::FsObserver;
use crate::filesystems::passthrough_fd::InodeMap;
use crate::vcs::IgnoreFilter;

/// A debug observer that logs filesystem operations to a file.
///
/// Each observed operation is formatted and appended to the log file
/// immediately (through a buffered writer). The observer holds its own
/// `Arc` clone of the shared [`InodeMap`] for inode-to-path resolution.
pub struct DumpObserver {
    /// Shared inode-to-path mapping (cloned Arc from the filesystem).
    inode_map: InodeMap,

    /// Root of the source directory — used for real-path lookups
    /// (e.g. distinguishing symlinks from regular files on unlink).
    source_dir: PathBuf,

    /// Monotonic sequence counter so every logged entry gets a number.
    next_seq: AtomicU64,

    /// Buffered, mutex-protected log writer.
    writer: Mutex<BufWriter<File>>,

    /// Ignore filters — paths matching any filter are silently dropped.
    ignore_filters: Vec<Box<dyn IgnoreFilter>>,
}

impl DumpObserver {
    /// Open (or create) a dump log file and return a ready-to-use observer.
    ///
    /// The file is opened in **append** mode so successive runs accumulate.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the log file cannot be opened.
    pub fn open(
        inode_map: InodeMap,
        source_dir: PathBuf,
        log_path: &Path,
    ) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        Ok(Self {
            inode_map,
            source_dir,
            next_seq: AtomicU64::new(1),
            writer: Mutex::new(BufWriter::new(file)),
            ignore_filters: Vec::new(),
        })
    }

    /// Attach ignore filters (builder-style).
    ///
    /// Paths matching any filter are silently dropped before logging.
    pub fn with_ignore_filters(mut self, filters: Vec<Box<dyn IgnoreFilter>>) -> Self {
        self.ignore_filters = filters;
        self
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Bump and return the next sequence number.
    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Resolve an inode to its relative path.
    fn resolve_inode(&self, ino: u64) -> Option<PathBuf> {
        self.inode_map.read().ok()?.get(&ino).cloned()
    }

    /// Resolve a parent inode + child name to a relative path.
    fn resolve_with_name(&self, parent: u64, name: &OsStr) -> Option<PathBuf> {
        self.resolve_inode(parent).map(|p| p.join(name))
    }

    /// Convert a relative path to the real (source) path.
    fn to_real(&self, rel_path: &Path) -> PathBuf {
        self.source_dir.join(rel_path)
    }

    /// Check whether a path should be ignored.
    fn is_ignored(&self, path: &Path) -> bool {
        self.ignore_filters.iter().any(|f| f.should_ignore(path))
    }

    /// Format a timestamp as seconds since the Unix epoch.
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Write a formatted entry to the log file.
    ///
    /// This acquires the writer lock, writes the header + body, and flushes
    /// the buffer so the entry is visible on disk promptly.
    fn log_entry(&self, body: &str) {
        let seq = self.next_seq();
        let ts = Self::now_secs();

        let mut w = match self.writer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("DumpObserver: writer lock poisoned, recovering");
                poisoned.into_inner()
            }
        };

        // Header
        let _ = writeln!(w, "═══════════════════════════════════════════════════════");
        let _ = writeln!(w, "Dump #{seq} (timestamp: {ts})");
        let _ = writeln!(w, "───────────────────────────────────────────────────────");

        // Body (pre-formatted by caller)
        let _ = write!(w, "{body}");
        let _ = writeln!(w);

        // Flush so tail -f works nicely
        let _ = w.flush();
    }

    /// Format at most `max_bytes` of `data` as a content preview.
    fn format_bytes(data: &[u8], max_bytes: usize) -> String {
        if data.is_empty() {
            return String::new();
        }

        let truncated = data.len() > max_bytes;
        let display = if truncated { &data[..max_bytes] } else { data };
        let suffix = if truncated { "..." } else { "" };

        if let Ok(s) = std::str::from_utf8(display) {
            format!("  Content (utf8): {s:?}{suffix}\n")
        } else {
            format!("  Bytes (non-utf8): {display:?}{suffix}\n")
        }
    }
}

// ---------------------------------------------------------------------------
// FsObserver implementation
// ---------------------------------------------------------------------------

impl FsObserver for DumpObserver {
    fn on_write(&self, ino: u64, _fh: u64, offset: i64, data: &[u8]) {
        let path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_write: unresolved inode {ino}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        let preview = Self::format_bytes(data, 100);
        self.log_entry(&format!(
            "  Type: FileWrite\n\
             \x20 Path: {path:?}\n\
             \x20 Offset: {offset}\n\
             \x20 Data: {} bytes\n\
             {preview}",
            data.len(),
        ));
    }

    fn on_create(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_create: unresolved parent {parent}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        self.log_entry(&format!(
            "  Type: FileCreate\n\
             \x20 Path: {path:?}\n\
             \x20 Mode: {mode:o}\n\
             \x20 Content: 0 bytes",
        ));
    }

    fn on_unlink(&self, parent: u64, name: &OsStr) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_unlink: unresolved parent {parent}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        let real_path = self.to_real(&path);
        let is_symlink = std::fs::symlink_metadata(&real_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        let kind = if is_symlink {
            "SymlinkDelete"
        } else {
            "FileDelete"
        };
        self.log_entry(&format!(
            "  Type: {kind}\n\
             \x20 Path: {path:?}",
        ));
    }

    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_mkdir: unresolved parent {parent}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        self.log_entry(&format!(
            "  Type: DirCreate\n\
             \x20 Path: {path:?}\n\
             \x20 Mode: {mode:o}",
        ));
    }

    fn on_rmdir(&self, parent: u64, name: &OsStr) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_rmdir: unresolved parent {parent}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        self.log_entry(&format!(
            "  Type: DirDelete\n\
             \x20 Path: {path:?}",
        ));
    }

    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {
        let old_path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_rename: unresolved old parent {parent}");
                return;
            }
        };
        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_rename: unresolved new parent {newparent}");
                return;
            }
        };

        if self.is_ignored(&old_path) || self.is_ignored(&new_path) {
            return;
        }

        let real_old = self.to_real(&old_path);
        let is_dir = std::fs::metadata(&real_old)
            .map(|m| m.is_dir())
            .unwrap_or(false);

        let kind = if is_dir { "DirRename" } else { "FileRename" };
        self.log_entry(&format!(
            "  Type: {kind}\n\
             \x20 Old Path: {old_path:?}\n\
             \x20 New Path: {new_path:?}",
        ));
    }

    fn on_setattr(
        &self,
        ino: u64,
        size: Option<u64>,
        mode: Option<u32>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) {
        let path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_setattr: unresolved inode {ino}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        if let Some(new_size) = size {
            self.log_entry(&format!(
                "  Type: FileTruncate\n\
                 \x20 Path: {path:?}\n\
                 \x20 New Size: {new_size}",
            ));
        }

        if let Some(new_mode) = mode {
            self.log_entry(&format!(
                "  Type: SetPermissions\n\
                 \x20 Path: {path:?}\n\
                 \x20 Mode: {new_mode:o}",
            ));
        }

        if atime.is_some() || mtime.is_some() {
            let atime_secs =
                atime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()));
            let mtime_secs =
                mtime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()));
            self.log_entry(&format!(
                "  Type: SetTimestamps\n\
                 \x20 Path: {path:?}\n\
                 \x20 Atime: {atime_secs:?}\n\
                 \x20 Mtime: {mtime_secs:?}",
            ));
        }
    }

    fn on_symlink(&self, parent: u64, name: &OsStr, target: &Path) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_symlink: unresolved parent {parent}");
                return;
            }
        };
        if self.is_ignored(&path) {
            return;
        }

        self.log_entry(&format!(
            "  Type: SymlinkCreate\n\
             \x20 Path: {path:?}\n\
             \x20 Target: {target:?}",
        ));
    }

    fn on_link(&self, ino: u64, newparent: u64, newname: &OsStr) {
        let existing_path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_link: unresolved inode {ino}");
                return;
            }
        };
        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                debug!("DumpObserver::on_link: unresolved new parent {newparent}");
                return;
            }
        };

        if self.is_ignored(&existing_path) || self.is_ignored(&new_path) {
            return;
        }

        self.log_entry(&format!(
            "  Type: HardLinkCreate\n\
             \x20 Existing Path: {existing_path:?}\n\
             \x20 New Path: {new_path:?}",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    /// Build a temporary DumpObserver writing to a temp file, with a
    /// pre-populated inode map.
    fn setup_dump_observer(
        entries: Vec<(u64, PathBuf)>,
    ) -> (DumpObserver, tempfile::NamedTempFile) {
        let map: HashMap<u64, PathBuf> = entries.into_iter().collect();
        let inode_map = Arc::new(RwLock::new(map));

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let observer =
            DumpObserver::open(inode_map, PathBuf::from("/tmp/source"), tmp.path()).unwrap();
        (observer, tmp)
    }

    fn read_log(tmp: &tempfile::NamedTempFile) -> String {
        std::fs::read_to_string(tmp.path()).unwrap()
    }

    #[test]
    fn test_on_write_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![(10, PathBuf::from("hello.txt"))]);

        obs.on_write(10, 0, 0, b"hello world");

        let log = read_log(&tmp);
        assert!(log.contains("FileWrite"), "expected FileWrite in log");
        assert!(log.contains("hello.txt"), "expected path in log");
        assert!(log.contains("11 bytes"), "expected byte count in log");
        assert!(
            log.contains("hello world"),
            "expected content preview in log"
        );
    }

    #[test]
    fn test_on_create_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from("src"))]);

        obs.on_create(1, OsStr::new("main.rs"), 0o644, Some(20));

        let log = read_log(&tmp);
        assert!(log.contains("FileCreate"));
        assert!(log.contains("main.rs"));
        assert!(log.contains("644"));
    }

    #[test]
    fn test_on_mkdir_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from(""))]);

        obs.on_mkdir(1, OsStr::new("subdir"), 0o755, Some(30));

        let log = read_log(&tmp);
        assert!(log.contains("DirCreate"));
        assert!(log.contains("subdir"));
        assert!(log.contains("755"));
    }

    #[test]
    fn test_on_rmdir_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from(""))]);

        obs.on_rmdir(1, OsStr::new("old_dir"));

        let log = read_log(&tmp);
        assert!(log.contains("DirDelete"));
        assert!(log.contains("old_dir"));
    }

    #[test]
    fn test_unresolved_inode_skipped() {
        let (obs, tmp) = setup_dump_observer(vec![]);

        obs.on_write(999, 0, 0, b"ghost");

        let log = read_log(&tmp);
        assert!(log.is_empty(), "no output expected for unresolved inode");
    }

    #[test]
    fn test_ignored_path_skipped() {
        use crate::vcs::GitBackend;

        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from(""))]);
        let obs = obs.with_ignore_filters(vec![Box::new(GitBackend)]);

        obs.on_create(1, OsStr::new(".git"), 0o755, None);

        let log = read_log(&tmp);
        assert!(log.is_empty(), "ignored path should produce no output");
    }

    #[test]
    fn test_sequence_numbers_increment() {
        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from(""))]);

        obs.on_mkdir(1, OsStr::new("a"), 0o755, None);
        obs.on_mkdir(1, OsStr::new("b"), 0o755, None);
        obs.on_mkdir(1, OsStr::new("c"), 0o755, None);

        let log = read_log(&tmp);
        assert!(log.contains("Dump #1"));
        assert!(log.contains("Dump #2"));
        assert!(log.contains("Dump #3"));
    }

    #[test]
    fn test_on_setattr_truncate() {
        let (obs, tmp) = setup_dump_observer(vec![(10, PathBuf::from("file.txt"))]);

        obs.on_setattr(10, Some(42), None, None, None);

        let log = read_log(&tmp);
        assert!(log.contains("FileTruncate"));
        assert!(log.contains("42"));
    }

    #[test]
    fn test_on_setattr_chmod() {
        let (obs, tmp) = setup_dump_observer(vec![(10, PathBuf::from("file.txt"))]);

        obs.on_setattr(10, None, Some(0o755), None, None);

        let log = read_log(&tmp);
        assert!(log.contains("SetPermissions"));
        assert!(log.contains("755"));
    }

    #[test]
    fn test_on_rename_logs_entry() {
        let (obs, tmp) =
            setup_dump_observer(vec![(1, PathBuf::from("src")), (2, PathBuf::from("dst"))]);

        obs.on_rename(1, OsStr::new("a.txt"), 2, OsStr::new("b.txt"));

        let log = read_log(&tmp);
        // The real path won't exist so metadata check defaults to file
        assert!(log.contains("FileRename"));
        assert!(log.contains("a.txt"));
        assert!(log.contains("b.txt"));
    }

    #[test]
    fn test_on_symlink_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![(1, PathBuf::from(""))]);

        obs.on_symlink(1, OsStr::new("link"), Path::new("/etc/hosts"));

        let log = read_log(&tmp);
        assert!(log.contains("SymlinkCreate"));
        assert!(log.contains("link"));
        assert!(log.contains("/etc/hosts"));
    }

    #[test]
    fn test_on_link_logs_entry() {
        let (obs, tmp) = setup_dump_observer(vec![
            (10, PathBuf::from("original.txt")),
            (1, PathBuf::from("")),
        ]);

        obs.on_link(10, 1, OsStr::new("hardlink.txt"));

        let log = read_log(&tmp);
        assert!(log.contains("HardLinkCreate"));
        assert!(log.contains("original.txt"));
        assert!(log.contains("hardlink.txt"));
    }

    #[test]
    fn test_format_bytes_utf8() {
        let s = DumpObserver::format_bytes(b"hello", 100);
        assert!(s.contains("utf8"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn test_format_bytes_truncated() {
        let data = b"abcdefghij";
        let s = DumpObserver::format_bytes(data, 5);
        assert!(s.contains("..."));
    }

    #[test]
    fn test_format_bytes_empty() {
        let s = DumpObserver::format_bytes(b"", 100);
        assert!(s.is_empty());
    }

    #[test]
    fn test_format_bytes_non_utf8() {
        let data: &[u8] = &[0xff, 0xfe, 0xfd];
        let s = DumpObserver::format_bytes(data, 100);
        assert!(s.contains("non-utf8"));
    }
}
