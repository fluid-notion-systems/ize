//! Opcode recorder that implements FsObserver.
//!
//! The `OpcodeRecorder` bridges filesystem observations to the opcode queue.
//! It receives notifications from `ObservingFS`, translates inodes to paths,
//! constructs `Opcode` instances, and enqueues them for async processing.
//!
//! # Example
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use std::path::PathBuf;
//! # use ize_lib::filesystems::passthrough::PassthroughFS;
//! # use ize_lib::filesystems::observing::ObservingFS;
//! # use ize_lib::operations::{OpcodeQueue, OpcodeRecorder};
//! # fn main() -> std::io::Result<()> {
//! # let source_dir = PathBuf::from("/tmp/source");
//! # let mount_point = PathBuf::from("/tmp/mount");
//! let passthrough = PassthroughFS::new(&source_dir, &mount_point)?;
//! let inode_map = passthrough.inode_map();
//! let queue = OpcodeQueue::new();
//!
//! let recorder = OpcodeRecorder::new(
//!     inode_map,
//!     source_dir.clone(),
//!     queue.sender(),
//! );
//!
//! let mut observing = ObservingFS::new(passthrough);
//! observing.add_observer(Arc::new(recorder));
//! # Ok(())
//! # }
//! ```

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{debug, warn};

use crate::filesystems::observing::FsObserver;
use crate::filesystems::passthrough::InodeMap;
use crate::operations::{Opcode, Operation};
use crate::vcs::IgnoreFilter;

use super::queue::OpcodeSender;

/// Records filesystem operations as opcodes.
///
/// Implements `FsObserver` to receive notifications from `ObservingFS`,
/// translates inodes to paths using a shared `InodeMap`, and enqueues
/// opcodes for async processing.
pub struct OpcodeRecorder {
    /// Shared inode-to-path mapping from PassthroughFS
    inode_map: InodeMap,

    /// Path to the source directory (for metadata lookups)
    source_dir: PathBuf,

    /// Monotonic sequence number generator
    next_seq: AtomicU64,

    /// Queue sender for enqueuing opcodes
    sender: OpcodeSender,

    /// Ignore filters — paths matching any filter are silently dropped.
    ignore_filters: Vec<Box<dyn IgnoreFilter>>,
}

impl OpcodeRecorder {
    /// Create a new opcode recorder.
    ///
    /// # Arguments
    /// * `inode_map` - Shared inode-to-path mapping from PassthroughFS
    /// * `source_dir` - Path to the source directory (for metadata lookups)
    /// * `sender` - Queue sender for enqueuing opcodes
    pub fn new(inode_map: InodeMap, source_dir: PathBuf, sender: OpcodeSender) -> Self {
        Self {
            inode_map,
            source_dir,
            next_seq: AtomicU64::new(1),
            sender,
            ignore_filters: Vec::new(),
        }
    }

    /// Set ignore filters for path-based filtering.
    ///
    /// Paths matching any filter will be silently dropped before recording.
    /// This is the primary mechanism for excluding VCS directories (.git, .jj, .pijul)
    /// and other managed paths from the opcode stream.
    pub fn with_ignore_filters(mut self, filters: Vec<Box<dyn IgnoreFilter>>) -> Self {
        self.ignore_filters = filters;
        self
    }

    /// Generate the next sequence number.
    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::SeqCst)
    }

    /// Get the current sequence number (for testing).
    #[cfg(test)]
    fn current_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst)
    }

    /// Resolve an inode to its relative path.
    fn resolve_inode(&self, ino: u64) -> Option<PathBuf> {
        self.inode_map.read().ok()?.get(&ino).cloned()
    }

    /// Resolve a parent inode and name to a relative path.
    fn resolve_with_name(&self, parent: u64, name: &OsStr) -> Option<PathBuf> {
        self.resolve_inode(parent).map(|p| p.join(name))
    }

    /// Convert a relative path to the real (source) path.
    fn to_real(&self, rel_path: &PathBuf) -> PathBuf {
        self.source_dir.join(rel_path)
    }

    /// Check whether a path should be ignored (not recorded).
    fn is_ignored(&self, path: &Path) -> bool {
        self.ignore_filters.iter().any(|f| f.should_ignore(path))
    }

    /// Emit an opcode to the queue.
    fn emit(&self, op: Operation) {
        let seq = self.next_seq();
        debug!("OpcodeRecorder::emit seq={} op={:?}", seq, op);
        let opcode = Opcode::new(seq, op);
        if let Err(_opcode) = self.sender.try_send(opcode) {
            warn!("Failed to enqueue opcode: queue at capacity");
            // Fallback: force push to avoid losing the opcode
            // self.sender.send(_opcode);
        }
    }
}

impl FsObserver for OpcodeRecorder {
    fn on_write(&self, ino: u64, _fh: u64, offset: i64, data: &[u8]) {
        debug!(
            "OpcodeRecorder::on_write(ino={}, offset={}, data_len={})",
            ino,
            offset,
            data.len()
        );
        let path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                warn!("on_write: failed to resolve inode {}", ino);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_write ignored path={:?}", path);
            return;
        }
        debug!("OpcodeRecorder::on_write resolved path={:?}", path);
        self.emit(Operation::FileWrite {
            path,
            offset: offset as u64,
            data: data.to_vec(),
        });
    }

    fn on_create(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        debug!(
            "OpcodeRecorder::on_create(parent={}, name={:?}, mode={:o})",
            parent, name, mode
        );
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_create: failed to resolve parent {}", parent);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_create ignored path={:?}", path);
            return;
        }
        debug!("OpcodeRecorder::on_create resolved path={:?}", path);
        self.emit(Operation::FileCreate {
            path,
            mode,
            content: Vec::new(), // Content will come via on_write
        });
    }

    fn on_unlink(&self, parent: u64, name: &OsStr) {
        debug!(
            "OpcodeRecorder::on_unlink(parent={}, name={:?})",
            parent, name
        );
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_unlink: failed to resolve parent {}", parent);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_unlink ignored path={:?}", path);
            return;
        }

        // Check if it's a symlink (use symlink_metadata to not follow symlinks)
        let real_path = self.to_real(&path);
        let is_symlink = std::fs::symlink_metadata(&real_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        debug!(
            "OpcodeRecorder::on_unlink resolved path={:?}, is_symlink={}",
            path, is_symlink
        );
        if is_symlink {
            self.emit(Operation::SymlinkDelete { path });
        } else {
            self.emit(Operation::FileDelete { path });
        }
    }

    fn on_mkdir(&self, parent: u64, name: &OsStr, mode: u32, _result_ino: Option<u64>) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_mkdir: failed to resolve parent {}", parent);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_mkdir ignored path={:?}", path);
            return;
        }

        self.emit(Operation::DirCreate { path, mode });
    }

    fn on_rmdir(&self, parent: u64, name: &OsStr) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_rmdir: failed to resolve parent {}", parent);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_rmdir ignored path={:?}", path);
            return;
        }

        self.emit(Operation::DirDelete { path });
    }

    fn on_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {
        let old_path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_rename: failed to resolve old parent {}", parent);
                return;
            }
        };

        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                warn!("on_rename: failed to resolve new parent {}", newparent);
                return;
            }
        };

        // Ignore if EITHER path is ignored (conservative — keeps VCS completely transparent)
        if self.is_ignored(&old_path) || self.is_ignored(&new_path) {
            debug!(
                "OpcodeRecorder::on_rename ignored old={:?} new={:?}",
                old_path, new_path
            );
            return;
        }

        // Check if source is a directory
        let real_old = self.to_real(&old_path);
        let is_dir = std::fs::metadata(&real_old)
            .map(|m| m.is_dir())
            .unwrap_or(false);

        if is_dir {
            self.emit(Operation::DirRename { old_path, new_path });
        } else {
            self.emit(Operation::FileRename { old_path, new_path });
        }
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
                warn!("on_setattr: failed to resolve inode {}", ino);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_setattr ignored path={:?}", path);
            return;
        }

        // Emit separate opcodes for each attribute change
        if let Some(new_size) = size {
            self.emit(Operation::FileTruncate {
                path: path.clone(),
                new_size,
            });
        }

        if let Some(new_mode) = mode {
            self.emit(Operation::SetPermissions {
                path: path.clone(),
                mode: new_mode,
            });
        }

        if atime.is_some() || mtime.is_some() {
            self.emit(Operation::SetTimestamps {
                path,
                atime: atime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())),
                mtime: mtime.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())),
            });
        }
    }

    fn on_symlink(&self, parent: u64, name: &OsStr, target: &std::path::Path) {
        let path = match self.resolve_with_name(parent, name) {
            Some(p) => p,
            None => {
                warn!("on_symlink: failed to resolve parent {}", parent);
                return;
            }
        };

        if self.is_ignored(&path) {
            debug!("OpcodeRecorder::on_symlink ignored path={:?}", path);
            return;
        }

        self.emit(Operation::SymlinkCreate {
            path,
            target: target.to_path_buf(),
        });
    }

    fn on_link(&self, ino: u64, newparent: u64, newname: &OsStr) {
        let existing_path = match self.resolve_inode(ino) {
            Some(p) => p,
            None => {
                warn!("on_link: failed to resolve inode {}", ino);
                return;
            }
        };

        let new_path = match self.resolve_with_name(newparent, newname) {
            Some(p) => p,
            None => {
                warn!("on_link: failed to resolve new parent {}", newparent);
                return;
            }
        };

        // Ignore if EITHER path is ignored (conservative)
        if self.is_ignored(&existing_path) || self.is_ignored(&new_path) {
            debug!(
                "OpcodeRecorder::on_link ignored existing={:?} new={:?}",
                existing_path, new_path
            );
            return;
        }

        self.emit(Operation::HardLinkCreate {
            existing_path,
            new_path,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::OpcodeQueue;
    use crate::vcs::GitBackend;
    use std::collections::HashMap;
    use std::sync::RwLock;

    fn setup_test_recorder() -> (OpcodeRecorder, std::sync::Arc<OpcodeQueue>) {
        let inode_map = std::sync::Arc::new(RwLock::new(HashMap::new()));

        // Set up some test inode mappings
        {
            let mut map = inode_map.write().unwrap();
            map.insert(1, PathBuf::from("")); // root
            map.insert(2, PathBuf::from("file.txt"));
            map.insert(3, PathBuf::from("dir"));
            map.insert(4, PathBuf::from("dir/subfile.txt"));
        }

        let source_dir = PathBuf::from("/tmp/test_source");
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        let recorder = OpcodeRecorder::new(inode_map, source_dir, sender);

        (recorder, queue)
    }

    fn setup_test_recorder_with_git_filter() -> (OpcodeRecorder, std::sync::Arc<OpcodeQueue>) {
        let inode_map = std::sync::Arc::new(RwLock::new(HashMap::new()));

        {
            let mut map = inode_map.write().unwrap();
            map.insert(1, PathBuf::from("")); // root
            map.insert(2, PathBuf::from("file.txt"));
            map.insert(3, PathBuf::from(".git"));
            map.insert(4, PathBuf::from(".git/objects"));
            map.insert(5, PathBuf::from(".git/index"));
            map.insert(6, PathBuf::from("src"));
            map.insert(7, PathBuf::from("src/main.rs"));
        }

        let source_dir = PathBuf::from("/tmp/test_source");
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        let filters: Vec<Box<dyn IgnoreFilter>> = vec![Box::new(GitBackend)];
        let recorder =
            OpcodeRecorder::new(inode_map, source_dir, sender).with_ignore_filters(filters);

        (recorder, queue)
    }

    #[test]
    fn test_recorder_creation() {
        let (recorder, _queue) = setup_test_recorder();
        assert_eq!(recorder.current_seq(), 1);
    }

    #[test]
    fn test_on_write() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_write(2, 1, 100, b"hello world");

        let opcode = queue.try_pop().unwrap();
        assert_eq!(opcode.seq(), 1);

        match opcode.into_op() {
            Operation::FileWrite { path, offset, data } => {
                assert_eq!(path, PathBuf::from("file.txt"));
                assert_eq!(offset, 100);
                assert_eq!(data, b"hello world");
            }
            _ => panic!("Expected FileWrite operation"),
        }
    }

    #[test]
    fn test_on_create() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_create(1, OsStr::new("new.txt"), 0o644, Some(10));

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::FileCreate {
                path,
                mode,
                content,
            } => {
                assert_eq!(path, PathBuf::from("new.txt"));
                assert_eq!(mode, 0o644);
                assert!(content.is_empty());
            }
            _ => panic!("Expected FileCreate operation"),
        }
    }

    #[test]
    fn test_on_unlink() {
        let (recorder, queue) = setup_test_recorder();

        // This will check the filesystem, but since the file doesn't exist,
        // it will default to FileDelete (not symlink)
        recorder.on_unlink(1, OsStr::new("file.txt"));

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::FileDelete { path } => {
                assert_eq!(path, PathBuf::from("file.txt"));
            }
            _ => panic!("Expected FileDelete operation"),
        }
    }

    #[test]
    fn test_on_mkdir() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_mkdir(1, OsStr::new("newdir"), 0o755, Some(20));

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::DirCreate { path, mode } => {
                assert_eq!(path, PathBuf::from("newdir"));
                assert_eq!(mode, 0o755);
            }
            _ => panic!("Expected DirCreate operation"),
        }
    }

    #[test]
    fn test_on_rmdir() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_rmdir(1, OsStr::new("dir"));

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::DirDelete { path } => {
                assert_eq!(path, PathBuf::from("dir"));
            }
            _ => panic!("Expected DirDelete operation"),
        }
    }

    #[test]
    fn test_on_rename() {
        let (recorder, queue) = setup_test_recorder();

        // Rename file.txt to renamed.txt (both under root)
        // Since the file doesn't exist on disk, metadata check will fail
        // and it will default to FileRename
        recorder.on_rename(1, OsStr::new("file.txt"), 1, OsStr::new("renamed.txt"));

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::FileRename { old_path, new_path } => {
                assert_eq!(old_path, PathBuf::from("file.txt"));
                assert_eq!(new_path, PathBuf::from("renamed.txt"));
            }
            _ => panic!("Expected FileRename operation"),
        }
    }

    #[test]
    fn test_on_setattr_truncate() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_setattr(2, Some(100), None, None, None);

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::FileTruncate { path, new_size } => {
                assert_eq!(path, PathBuf::from("file.txt"));
                assert_eq!(new_size, 100);
            }
            _ => panic!("Expected FileTruncate operation"),
        }
    }

    #[test]
    fn test_on_setattr_chmod() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_setattr(2, None, Some(0o600), None, None);

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::SetPermissions { path, mode } => {
                assert_eq!(path, PathBuf::from("file.txt"));
                assert_eq!(mode, 0o600);
            }
            _ => panic!("Expected SetPermissions operation"),
        }
    }

    #[test]
    fn test_on_setattr_multiple() {
        let (recorder, queue) = setup_test_recorder();

        // Set both size and mode - should emit two opcodes
        recorder.on_setattr(2, Some(50), Some(0o755), None, None);

        let op1 = queue.try_pop().unwrap();
        let op2 = queue.try_pop().unwrap();

        assert!(matches!(op1.op(), Operation::FileTruncate { .. }));
        assert!(matches!(op2.op(), Operation::SetPermissions { .. }));
    }

    #[test]
    fn test_sequence_numbers_increment() {
        let (recorder, queue) = setup_test_recorder();

        recorder.on_mkdir(1, OsStr::new("dir1"), 0o755, None);
        recorder.on_mkdir(1, OsStr::new("dir2"), 0o755, None);
        recorder.on_mkdir(1, OsStr::new("dir3"), 0o755, None);

        let op1 = queue.try_pop().unwrap();
        let op2 = queue.try_pop().unwrap();
        let op3 = queue.try_pop().unwrap();

        assert_eq!(op1.seq(), 1);
        assert_eq!(op2.seq(), 2);
        assert_eq!(op3.seq(), 3);
    }

    #[test]
    fn test_unresolved_inode_skipped() {
        let (recorder, queue) = setup_test_recorder();

        // Inode 999 doesn't exist in our map
        recorder.on_write(999, 1, 0, b"data");

        // Should not have enqueued anything
        assert!(queue.is_empty());
    }

    #[test]
    fn test_nested_path_resolution() {
        let (recorder, queue) = setup_test_recorder();

        // Create a file under dir (inode 3)
        recorder.on_create(3, OsStr::new("nested.txt"), 0o644, None);

        let opcode = queue.try_pop().unwrap();
        match opcode.into_op() {
            Operation::FileCreate { path, .. } => {
                assert_eq!(path, PathBuf::from("dir/nested.txt"));
            }
            _ => panic!("Expected FileCreate operation"),
        }
    }

    // =========================================================================
    // IgnoreFilter tests
    // =========================================================================

    #[test]
    fn test_filter_ignores_git_write() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        // Write to .git/index — should be ignored
        recorder.on_write(5, 1, 0, b"data");
        assert!(queue.is_empty(), ".git write should be filtered");

        // Write to regular file — should be recorded
        recorder.on_write(2, 1, 0, b"data");
        assert!(!queue.is_empty(), "regular write should be recorded");
    }

    #[test]
    fn test_filter_ignores_git_create() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        // Create inside .git — should be ignored
        recorder.on_create(3, OsStr::new("new_object"), 0o644, None);
        assert!(queue.is_empty(), ".git create should be filtered");

        // Create in regular dir — should be recorded
        recorder.on_create(6, OsStr::new("lib.rs"), 0o644, None);
        assert!(!queue.is_empty(), "regular create should be recorded");
    }

    #[test]
    fn test_filter_ignores_git_unlink() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        recorder.on_unlink(3, OsStr::new("index.lock"));
        assert!(queue.is_empty(), ".git unlink should be filtered");

        recorder.on_unlink(1, OsStr::new("file.txt"));
        assert!(!queue.is_empty(), "regular unlink should be recorded");
    }

    #[test]
    fn test_filter_ignores_git_mkdir() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        recorder.on_mkdir(3, OsStr::new("refs"), 0o755, None);
        assert!(queue.is_empty(), ".git mkdir should be filtered");

        recorder.on_mkdir(1, OsStr::new("src"), 0o755, None);
        assert!(!queue.is_empty(), "regular mkdir should be recorded");
    }

    #[test]
    fn test_filter_ignores_git_rmdir() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        recorder.on_rmdir(3, OsStr::new("objects"));
        assert!(queue.is_empty(), ".git rmdir should be filtered");

        recorder.on_rmdir(1, OsStr::new("dir"));
        assert!(!queue.is_empty(), "regular rmdir should be recorded");
    }

    #[test]
    fn test_filter_ignores_git_rename_either_path() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        // Rename within .git — ignored
        recorder.on_rename(3, OsStr::new("old"), 3, OsStr::new("new"));
        assert!(queue.is_empty(), ".git→.git rename should be filtered");

        // Rename from .git to regular — ignored (conservative: EITHER path)
        recorder.on_rename(3, OsStr::new("leaked"), 1, OsStr::new("leaked"));
        assert!(queue.is_empty(), ".git→regular rename should be filtered");

        // Rename from regular to .git — ignored
        recorder.on_rename(1, OsStr::new("file.txt"), 3, OsStr::new("stashed"));
        assert!(queue.is_empty(), "regular→.git rename should be filtered");

        // Rename between regular dirs — recorded
        recorder.on_rename(1, OsStr::new("file.txt"), 6, OsStr::new("moved.txt"));
        assert!(
            !queue.is_empty(),
            "regular→regular rename should be recorded"
        );
    }

    #[test]
    fn test_filter_ignores_git_setattr() {
        let (recorder, queue) = setup_test_recorder_with_git_filter();

        // setattr on .git/index — ignored
        recorder.on_setattr(5, Some(100), None, None, None);
        assert!(queue.is_empty(), ".git setattr should be filtered");

        // setattr on regular file — recorded
        recorder.on_setattr(2, Some(100), None, None, None);
        assert!(!queue.is_empty(), "regular setattr should be recorded");
    }

    #[test]
    fn test_no_filters_records_everything() {
        let (recorder, queue) = setup_test_recorder();

        // Without filters, .git-like paths under "dir" inode are still recorded
        // (the default setup doesn't have .git paths, but we can create under root)
        recorder.on_create(1, OsStr::new(".git"), 0o755, None);
        assert!(!queue.is_empty(), "without filters everything is recorded");
    }

    #[test]
    fn test_with_ignore_filters_builder() {
        let inode_map = std::sync::Arc::new(RwLock::new(HashMap::new()));
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        let filters: Vec<Box<dyn IgnoreFilter>> = vec![Box::new(GitBackend)];
        let recorder = OpcodeRecorder::new(inode_map, PathBuf::from("/tmp"), sender)
            .with_ignore_filters(filters);

        assert!(!recorder.ignore_filters.is_empty());
    }
}
