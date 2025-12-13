//! Observing Filesystem Wrapper
//!
//! This module provides the Observer pattern for filesystem operations:
//! - `FsObserver` trait: Receives notifications about filesystem mutations
//! - `ObservingFS<F>`: Wraps any `Filesystem` and notifies observers of mutations
//!
//! The key insight is that we're not "fanning out" filesystem operations - we're
//! observing them. The actual filesystem operation only happens once in the inner
//! filesystem. Observers just receive notifications with relevant data.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                      FUSE Kernel                         │
//! └─────────────────────────┬───────────────────────────────┘
//!                           │
//!                           ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │                    ObservingFS<F>                        │
//! │  ┌─────────────────────────────────────────────────┐    │
//! │  │  observers: Vec<Arc<dyn FsObserver>>            │    │
//! │  └─────────────────────────────────────────────────┘    │
//! │                          │                               │
//! │                          ▼                               │
//! │  ┌─────────────────────────────────────────────────┐    │
//! │  │              inner: F (e.g., PassthroughFS)     │    │
//! │  └─────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────┘
//! ```

use std::ffi::OsStr;
use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use log::debug;

use fuser::{
    Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};

use super::passthrough::PassthroughFS;

/// Observer trait for filesystem mutations.
///
/// Implementors receive notifications about filesystem changes but do NOT handle
/// FUSE replies - that's the inner filesystem's job.
///
/// All methods have default empty implementations so observers can choose which
/// operations they care about.
///
/// # Thread Safety
///
/// Observers must be `Send + Sync` as they may be called from multiple threads.
/// Implementations should be non-blocking - use channels or queues for async work.
pub trait FsObserver: Send + Sync {
    /// Called when a write operation occurs.
    ///
    /// # Arguments
    /// * `ino` - Inode number of the file
    /// * `fh` - File handle
    /// * `offset` - Byte offset where write begins
    /// * `data` - The data being written
    fn on_write(&self, _ino: u64, _fh: u64, _offset: i64, _data: &[u8]) {}

    /// Called when a file is created.
    ///
    /// # Arguments
    /// * `parent` - Parent directory inode
    /// * `name` - Name of the new file
    /// * `mode` - File mode/permissions
    /// * `result_ino` - The inode of the created file (if known)
    fn on_create(&self, _parent: u64, _name: &OsStr, _mode: u32, _result_ino: Option<u64>) {}

    /// Called when a file is unlinked (deleted).
    ///
    /// # Arguments
    /// * `parent` - Parent directory inode
    /// * `name` - Name of the file being deleted
    fn on_unlink(&self, _parent: u64, _name: &OsStr) {}

    /// Called when a directory is created.
    ///
    /// # Arguments
    /// * `parent` - Parent directory inode
    /// * `name` - Name of the new directory
    /// * `mode` - Directory mode/permissions
    /// * `result_ino` - The inode of the created directory (if known)
    fn on_mkdir(&self, _parent: u64, _name: &OsStr, _mode: u32, _result_ino: Option<u64>) {}

    /// Called when a directory is removed.
    ///
    /// # Arguments
    /// * `parent` - Parent directory inode
    /// * `name` - Name of the directory being removed
    fn on_rmdir(&self, _parent: u64, _name: &OsStr) {}

    /// Called when a file or directory is renamed.
    ///
    /// # Arguments
    /// * `parent` - Original parent directory inode
    /// * `name` - Original name
    /// * `newparent` - New parent directory inode
    /// * `newname` - New name
    fn on_rename(&self, _parent: u64, _name: &OsStr, _newparent: u64, _newname: &OsStr) {}

    /// Called when file attributes are changed.
    ///
    /// # Arguments
    /// * `ino` - Inode of the file/directory
    /// * `size` - New size (if truncating)
    /// * `mode` - New mode/permissions
    /// * `atime` - New access time
    /// * `mtime` - New modification time
    fn on_setattr(
        &self,
        _ino: u64,
        _size: Option<u64>,
        _mode: Option<u32>,
        _atime: Option<SystemTime>,
        _mtime: Option<SystemTime>,
    ) {
    }

    /// Called when a symlink is created.
    ///
    /// # Arguments
    /// * `parent` - Parent directory inode
    /// * `name` - Name of the symlink
    /// * `target` - Target path the symlink points to
    fn on_symlink(&self, _parent: u64, _name: &OsStr, _target: &std::path::Path) {}

    /// Called when a hard link is created.
    ///
    /// # Arguments
    /// * `ino` - Inode of the existing file
    /// * `newparent` - Parent directory for the new link
    /// * `newname` - Name of the new link
    fn on_link(&self, _ino: u64, _newparent: u64, _newname: &OsStr) {}
}

/// A filesystem wrapper that notifies observers of mutation operations.
///
/// `ObservingFS` wraps any filesystem implementing `fuser::Filesystem` and
/// adds observation capabilities. Observers are notified of mutations BEFORE
/// the operation is delegated to the inner filesystem.
///
/// Read-only operations (lookup, getattr, read, readdir, etc.) are passed
/// directly to the inner filesystem without observer notification.
///
/// # Type Parameters
///
/// * `F` - The inner filesystem type (must implement `Filesystem`)
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # use std::path::PathBuf;
/// # use ize_lib::filesystems::passthrough::PassthroughFS;
/// # use ize_lib::filesystems::observing::{ObservingFS, FsObserver};
/// # fn main() -> std::io::Result<()> {
/// # let source_dir = PathBuf::from("/tmp/source");
/// # let mount_point = PathBuf::from("/tmp/mount");
/// let passthrough = PassthroughFS::new(&source_dir, &mount_point)?;
/// let mut observing = ObservingFS::new(passthrough);
///
/// // Add an observer (implement FsObserver trait)
/// // let my_observer = Arc::new(MyObserver::new());
/// // observing.add_observer(my_observer);
///
/// // Mount - observers will be notified of all mutations
/// // fuser::mount2(observing, mount_point, &options)?;
/// # Ok(())
/// # }
/// ```
pub struct ObservingFS<F: Filesystem> {
    /// The wrapped inner filesystem
    inner: F,
    /// List of observers to notify on mutations
    observers: Vec<Arc<dyn FsObserver>>,
}

impl<F: Filesystem> ObservingFS<F> {
    /// Create a new observing filesystem wrapper.
    ///
    /// # Arguments
    /// * `inner` - The filesystem to wrap
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            observers: Vec::new(),
        }
    }

    /// Add an observer to be notified of filesystem mutations.
    ///
    /// Observers are called in the order they were added.
    pub fn add_observer(&mut self, observer: Arc<dyn FsObserver>) {
        self.observers.push(observer);
    }

    /// Get a reference to the inner filesystem.
    pub fn inner(&self) -> &F {
        &self.inner
    }

    /// Get a mutable reference to the inner filesystem.
    pub fn inner_mut(&mut self) -> &mut F {
        &mut self.inner
    }
}

impl ObservingFS<PassthroughFS> {
    /// Mount the observing filesystem.
    ///
    /// This delegates to the inner PassthroughFS for mount point and read-only settings.
    pub fn mount(self) -> io::Result<()> {
        let mut options = vec![
            MountOption::FSName("ize".to_string()),
            MountOption::AutoUnmount,
            MountOption::AllowOther,
        ];

        if self.inner.is_read_only() {
            options.push(MountOption::RO);
        }

        let mount_point = self.inner.mount_point().to_path_buf();
        fuser::mount2(self, mount_point, &options)?;
        Ok(())
    }
}

impl<F: Filesystem> ObservingFS<F> {
    /// Notify all observers of a write operation.
    fn notify_write(&self, ino: u64, fh: u64, offset: i64, data: &[u8]) {
        debug!(
            "ObservingFS::notify_write(ino={}, fh={}, offset={}, data_len={})",
            ino,
            fh,
            offset,
            data.len()
        );
        for observer in &self.observers {
            observer.on_write(ino, fh, offset, data);
        }
    }

    /// Notify all observers of a create operation.
    fn notify_create(&self, parent: u64, name: &OsStr, mode: u32) {
        debug!(
            "ObservingFS::notify_create(parent={}, name={:?}, mode={:o})",
            parent, name, mode
        );
        for observer in &self.observers {
            observer.on_create(parent, name, mode, None);
        }
    }

    /// Notify all observers of an unlink operation.
    fn notify_unlink(&self, parent: u64, name: &OsStr) {
        debug!(
            "ObservingFS::notify_unlink(parent={}, name={:?})",
            parent, name
        );
        for observer in &self.observers {
            observer.on_unlink(parent, name);
        }
    }

    /// Notify all observers of a mkdir operation.
    fn notify_mkdir(&self, parent: u64, name: &OsStr, mode: u32) {
        debug!(
            "ObservingFS::notify_mkdir(parent={}, name={:?}, mode={:o})",
            parent, name, mode
        );
        for observer in &self.observers {
            observer.on_mkdir(parent, name, mode, None);
        }
    }

    /// Notify all observers of a rmdir operation.
    fn notify_rmdir(&self, parent: u64, name: &OsStr) {
        debug!(
            "ObservingFS::notify_rmdir(parent={}, name={:?})",
            parent, name
        );
        for observer in &self.observers {
            observer.on_rmdir(parent, name);
        }
    }

    /// Notify all observers of a rename operation.
    fn notify_rename(&self, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr) {
        debug!(
            "ObservingFS::notify_rename(parent={}, name={:?}, newparent={}, newname={:?})",
            parent, name, newparent, newname
        );
        for observer in &self.observers {
            observer.on_rename(parent, name, newparent, newname);
        }
    }

    /// Notify all observers of a setattr operation.
    fn notify_setattr(
        &self,
        ino: u64,
        size: Option<u64>,
        mode: Option<u32>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) {
        for observer in &self.observers {
            observer.on_setattr(ino, size, mode, atime, mtime);
        }
    }

    /// Notify all observers of a symlink operation.
    #[allow(dead_code)]
    fn notify_symlink(&self, parent: u64, name: &OsStr, target: &std::path::Path) {
        for observer in &self.observers {
            observer.on_symlink(parent, name, target);
        }
    }

    /// Notify all observers of a link operation.
    #[allow(dead_code)]
    fn notify_link(&self, ino: u64, newparent: u64, newname: &OsStr) {
        for observer in &self.observers {
            observer.on_link(ino, newparent, newname);
        }
    }
}

/// Helper to convert TimeOrNow to Option<SystemTime>
fn time_or_now_to_system_time(t: TimeOrNow) -> Option<SystemTime> {
    match t {
        TimeOrNow::SpecificTime(st) => Some(st),
        TimeOrNow::Now => Some(SystemTime::now()),
    }
}

impl<F: Filesystem> Filesystem for ObservingFS<F> {
    // =========================================================================
    // Lifecycle methods - delegate directly
    // =========================================================================

    fn init(
        &mut self,
        req: &Request<'_>,
        config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        self.inner.init(req, config)
    }

    fn destroy(&mut self) {
        self.inner.destroy()
    }

    // =========================================================================
    // Read-only operations - delegate directly, no observation needed
    // =========================================================================

    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        self.inner.lookup(req, parent, name, reply)
    }

    fn getattr(&mut self, req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        self.inner.getattr(req, ino, fh, reply)
    }

    fn readdir(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        reply: ReplyDirectory,
    ) {
        self.inner.readdir(req, ino, fh, offset, reply)
    }

    fn open(&mut self, req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        self.inner.open(req, ino, flags, reply)
    }

    fn read(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        self.inner
            .read(req, ino, fh, offset, size, flags, lock_owner, reply)
    }

    fn flush(&mut self, req: &Request<'_>, ino: u64, fh: u64, lock_owner: u64, reply: ReplyEmpty) {
        self.inner.flush(req, ino, fh, lock_owner, reply)
    }

    fn release(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        fh: u64,
        flags: i32,
        lock_owner: Option<u64>,
        flush: bool,
        reply: ReplyEmpty,
    ) {
        self.inner
            .release(req, ino, fh, flags, lock_owner, flush, reply)
    }

    fn fsync(&mut self, req: &Request<'_>, ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
        self.inner.fsync(req, ino, fh, datasync, reply)
    }

    fn opendir(&mut self, req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        self.inner.opendir(req, ino, flags, reply)
    }

    fn releasedir(&mut self, req: &Request<'_>, ino: u64, fh: u64, flags: i32, reply: ReplyEmpty) {
        self.inner.releasedir(req, ino, fh, flags, reply)
    }

    fn access(&mut self, req: &Request<'_>, ino: u64, mask: i32, reply: ReplyEmpty) {
        self.inner.access(req, ino, mask, reply)
    }

    fn statfs(&mut self, req: &Request<'_>, ino: u64, reply: ReplyStatfs) {
        self.inner.statfs(req, ino, reply)
    }

    // =========================================================================
    // Mutation operations - notify observers BEFORE delegating
    // =========================================================================

    fn write(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        write_flags: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // Notify observers first
        self.notify_write(ino, fh, offset, data);

        // Delegate to inner filesystem
        self.inner.write(
            req,
            ino,
            fh,
            offset,
            data,
            write_flags,
            flags,
            lock_owner,
            reply,
        )
    }

    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        // Notify observers first
        self.notify_create(parent, name, mode);

        // Delegate to inner filesystem
        self.inner
            .create(req, parent, name, mode, umask, flags, reply)
    }

    fn unlink(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        // Notify observers first
        self.notify_unlink(parent, name);

        // Delegate to inner filesystem
        self.inner.unlink(req, parent, name, reply)
    }

    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        // Notify observers first
        self.notify_mkdir(parent, name, mode);

        // Delegate to inner filesystem
        self.inner.mkdir(req, parent, name, mode, umask, reply)
    }

    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        // Notify observers first
        self.notify_rmdir(parent, name);

        // Delegate to inner filesystem
        self.inner.rmdir(req, parent, name, reply)
    }

    fn rename(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        // Notify observers first
        self.notify_rename(parent, name, newparent, newname);

        // Delegate to inner filesystem
        self.inner
            .rename(req, parent, name, newparent, newname, flags, reply)
    }

    fn setattr(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        fh: Option<u64>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // Notify observers first (convert TimeOrNow to SystemTime)
        let atime_st = atime.and_then(time_or_now_to_system_time);
        let mtime_st = mtime.and_then(time_or_now_to_system_time);
        self.notify_setattr(ino, size, mode, atime_st, mtime_st);

        // Delegate to inner filesystem
        self.inner.setattr(
            req, ino, mode, uid, gid, size, atime, mtime, ctime, fh, crtime, chgtime, bkuptime,
            flags, reply,
        )
    }

    // Note: symlink and link are not implemented in PassthroughFS currently,
    // but we include the observer hooks for future use.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test observer that counts calls
    struct CountingObserver {
        write_count: AtomicUsize,
        create_count: AtomicUsize,
        unlink_count: AtomicUsize,
        mkdir_count: AtomicUsize,
        rmdir_count: AtomicUsize,
        rename_count: AtomicUsize,
        setattr_count: AtomicUsize,
    }

    impl CountingObserver {
        fn new() -> Self {
            Self {
                write_count: AtomicUsize::new(0),
                create_count: AtomicUsize::new(0),
                unlink_count: AtomicUsize::new(0),
                mkdir_count: AtomicUsize::new(0),
                rmdir_count: AtomicUsize::new(0),
                rename_count: AtomicUsize::new(0),
                setattr_count: AtomicUsize::new(0),
            }
        }

        fn write_count(&self) -> usize {
            self.write_count.load(Ordering::SeqCst)
        }

        fn create_count(&self) -> usize {
            self.create_count.load(Ordering::SeqCst)
        }

        fn unlink_count(&self) -> usize {
            self.unlink_count.load(Ordering::SeqCst)
        }

        fn mkdir_count(&self) -> usize {
            self.mkdir_count.load(Ordering::SeqCst)
        }

        fn rmdir_count(&self) -> usize {
            self.rmdir_count.load(Ordering::SeqCst)
        }

        fn rename_count(&self) -> usize {
            self.rename_count.load(Ordering::SeqCst)
        }

        fn setattr_count(&self) -> usize {
            self.setattr_count.load(Ordering::SeqCst)
        }
    }

    impl FsObserver for CountingObserver {
        fn on_write(&self, _ino: u64, _fh: u64, _offset: i64, _data: &[u8]) {
            self.write_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_create(&self, _parent: u64, _name: &OsStr, _mode: u32, _result_ino: Option<u64>) {
            self.create_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_unlink(&self, _parent: u64, _name: &OsStr) {
            self.unlink_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_mkdir(&self, _parent: u64, _name: &OsStr, _mode: u32, _result_ino: Option<u64>) {
            self.mkdir_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_rmdir(&self, _parent: u64, _name: &OsStr) {
            self.rmdir_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_rename(&self, _parent: u64, _name: &OsStr, _newparent: u64, _newname: &OsStr) {
            self.rename_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_setattr(
            &self,
            _ino: u64,
            _size: Option<u64>,
            _mode: Option<u32>,
            _atime: Option<SystemTime>,
            _mtime: Option<SystemTime>,
        ) {
            self.setattr_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A minimal mock filesystem for testing
    struct MockFilesystem;

    impl Filesystem for MockFilesystem {
        // All methods use default implementations (which do nothing)
    }

    #[test]
    fn test_observing_fs_creation() {
        let mock = MockFilesystem;
        let observing = ObservingFS::new(mock);
        assert!(observing.observers.is_empty());
    }

    #[test]
    fn test_add_observer() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer);

        assert_eq!(observing.observers.len(), 1);
    }

    #[test]
    fn test_multiple_observers() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer1 = Arc::new(CountingObserver::new());
        let observer2 = Arc::new(CountingObserver::new());

        observing.add_observer(observer1);
        observing.add_observer(observer2);

        assert_eq!(observing.observers.len(), 2);
    }

    #[test]
    fn test_notify_write() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        // Directly call notify to test the notification mechanism
        observing.notify_write(1, 1, 0, b"hello");

        assert_eq!(observer.write_count(), 1);
    }

    #[test]
    fn test_notify_create() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_create(1, OsStr::new("test.txt"), 0o644);

        assert_eq!(observer.create_count(), 1);
    }

    #[test]
    fn test_notify_unlink() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_unlink(1, OsStr::new("test.txt"));

        assert_eq!(observer.unlink_count(), 1);
    }

    #[test]
    fn test_notify_mkdir() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_mkdir(1, OsStr::new("subdir"), 0o755);

        assert_eq!(observer.mkdir_count(), 1);
    }

    #[test]
    fn test_notify_rmdir() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_rmdir(1, OsStr::new("subdir"));

        assert_eq!(observer.rmdir_count(), 1);
    }

    #[test]
    fn test_notify_rename() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_rename(1, OsStr::new("old.txt"), 2, OsStr::new("new.txt"));

        assert_eq!(observer.rename_count(), 1);
    }

    #[test]
    fn test_notify_setattr() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer = Arc::new(CountingObserver::new());
        observing.add_observer(observer.clone());

        observing.notify_setattr(1, Some(1024), Some(0o644), None, None);

        assert_eq!(observer.setattr_count(), 1);
    }

    #[test]
    fn test_multiple_observers_all_notified() {
        let mock = MockFilesystem;
        let mut observing = ObservingFS::new(mock);

        let observer1 = Arc::new(CountingObserver::new());
        let observer2 = Arc::new(CountingObserver::new());

        observing.add_observer(observer1.clone());
        observing.add_observer(observer2.clone());

        // Trigger a write notification
        observing.notify_write(1, 1, 0, b"hello");

        // Both observers should be notified
        assert_eq!(observer1.write_count(), 1);
        assert_eq!(observer2.write_count(), 1);
    }
}
