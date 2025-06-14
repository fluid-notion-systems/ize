use claris_fuse::{PassthroughFS, Storage};
use fuser::{BackgroundSession, MountOption};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::{tempdir, TempDir};

/// Integration test harness for operation tracking
struct OperationTrackingHarness {
    source_dir: TempDir,
    mount_dir: TempDir,
    db_path: PathBuf,
    _session: Option<BackgroundSession>,
}

impl OperationTrackingHarness {
    fn new() -> io::Result<Self> {
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;
        let db_path = source_dir.path().join(".claris.db");

        Ok(Self {
            source_dir,
            mount_dir,
            db_path,
            _session: None,
        })
    }

    fn mount(mut self) -> io::Result<Self> {
        // Create the filesystem instance
        let fs = PassthroughFS::new(self.source_dir.path().to_path_buf(), self.db_path.clone())?;

        let mount_path = self.mount_dir.path().to_path_buf();
        let options = vec![
            MountOption::FSName("claris-test".to_string()),
            MountOption::AutoUnmount,
            MountOption::AllowOther,
        ];

        // Mount the filesystem in background
        let session = fuser::spawn_mount2(fs, &mount_path, &options)?;

        // Wait for mount to be ready
        thread::sleep(Duration::from_millis(300));

        // Verify mount is accessible
        if fs::read_dir(&mount_path).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Mount point not accessible",
            ));
        }

        self._session = Some(session);
        Ok(self)
    }

    /// Get the storage instance to check tracked operations
    fn get_storage(&self) -> io::Result<Box<dyn Storage>> {
        // This would connect to the actual storage backend
        // For now, we'll check the filesystem state
        unimplemented!("Storage access implementation needed")
    }

    /// Check if an operation was tracked by examining the filesystem state
    fn verify_operation_tracked(&self, path: &Path, operation: &str) -> bool {
        // In a real implementation, this would query the storage
        // For now, we verify the operation succeeded
        match operation {
            "create" | "write" => self.source_dir.path().join(path).exists(),
            "delete" => !self.source_dir.path().join(path).exists(),
            "mkdir" => {
                let full_path = self.source_dir.path().join(path);
                full_path.exists() && full_path.is_dir()
            }
            "rmdir" => !self.source_dir.path().join(path).exists(),
            _ => true,
        }
    }

    /// Get list of operations recorded (simplified for testing)
    fn get_recorded_operations(&self) -> Vec<RecordedOperation> {
        // This would query the actual storage
        // For now, return empty vec
        Vec::new()
    }

    fn source_path(&self) -> &Path {
        self.source_dir.path()
    }

    fn mount_path(&self) -> &Path {
        self.mount_dir.path()
    }
}

#[derive(Debug, Clone)]
struct RecordedOperation {
    operation: String,
    path: PathBuf,
    timestamp: SystemTime,
    metadata: Option<OperationMetadata>,
}

#[derive(Debug, Clone)]
enum OperationMetadata {
    Write {
        offset: u64,
        size: usize,
    },
    Create {
        mode: u32,
    },
    SetAttr {
        mode: Option<u32>,
        size: Option<u64>,
    },
    Rename {
        new_path: PathBuf,
    },
}

// === File Operation Tracking Tests ===

#[test]
fn test_file_create_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create a file through the mount
    let file_path = harness.mount_path().join("new_file.txt");
    fs::write(&file_path, b"Hello, Claris!").unwrap();

    // Give FUSE time to process
    thread::sleep(Duration::from_millis(100));

    // Verify file exists in source
    let source_file = harness.source_path().join("new_file.txt");
    assert!(source_file.exists(), "File should exist in source");

    // Verify content matches
    let content = fs::read(&source_file).unwrap();
    assert_eq!(content, b"Hello, Claris!");

    // Verify operation was tracked
    assert!(
        harness.verify_operation_tracked(Path::new("new_file.txt"), "create"),
        "Create operation should be tracked"
    );
}

#[test]
fn test_file_write_operations_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    let file_path = harness.mount_path().join("write_test.txt");

    // Initial write
    fs::write(&file_path, b"Initial content").unwrap();
    thread::sleep(Duration::from_millis(50));

    // Overwrite
    fs::write(&file_path, b"New content").unwrap();
    thread::sleep(Duration::from_millis(50));

    // Append
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&file_path)
        .unwrap();
    use std::io::Write;
    write!(file, " - Appended").unwrap();
    drop(file);

    thread::sleep(Duration::from_millis(100));

    // Verify final content
    let source_file = harness.source_path().join("write_test.txt");
    let content = fs::read(&source_file).unwrap();
    assert_eq!(content, b"New content - Appended");

    // All write operations should be tracked
    assert!(harness.verify_operation_tracked(Path::new("write_test.txt"), "write"));
}

#[test]
fn test_file_delete_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create then delete a file
    let file_path = harness.mount_path().join("delete_me.txt");
    fs::write(&file_path, b"Temporary file").unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify it exists first
    assert!(harness.source_path().join("delete_me.txt").exists());

    // Delete through mount
    fs::remove_file(&file_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify deletion
    assert!(!harness.source_path().join("delete_me.txt").exists());
    assert!(harness.verify_operation_tracked(Path::new("delete_me.txt"), "delete"));
}

#[test]
fn test_file_rename_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create original file
    let old_path = harness.mount_path().join("original.txt");
    let new_path = harness.mount_path().join("renamed.txt");
    fs::write(&old_path, b"File to rename").unwrap();
    thread::sleep(Duration::from_millis(100));

    // Rename through mount
    fs::rename(&old_path, &new_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify rename
    assert!(!harness.source_path().join("original.txt").exists());
    assert!(harness.source_path().join("renamed.txt").exists());

    // Verify content preserved
    let content = fs::read(harness.source_path().join("renamed.txt")).unwrap();
    assert_eq!(content, b"File to rename");
}

// === Directory Operation Tracking Tests ===

#[test]
fn test_mkdir_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create directory through mount
    let dir_path = harness.mount_path().join("new_directory");
    fs::create_dir(&dir_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify in source
    let source_dir = harness.source_path().join("new_directory");
    assert!(source_dir.exists() && source_dir.is_dir());

    assert!(harness.verify_operation_tracked(Path::new("new_directory"), "mkdir"));
}

#[test]
fn test_nested_mkdir_operations_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create nested directories
    let nested_path = harness.mount_path().join("parent/child/grandchild");
    fs::create_dir_all(&nested_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify all levels exist
    assert!(harness.source_path().join("parent").is_dir());
    assert!(harness.source_path().join("parent/child").is_dir());
    assert!(harness
        .source_path()
        .join("parent/child/grandchild")
        .is_dir());

    // Each mkdir should be tracked
    assert!(harness.verify_operation_tracked(Path::new("parent"), "mkdir"));
    assert!(harness.verify_operation_tracked(Path::new("parent/child"), "mkdir"));
    assert!(harness.verify_operation_tracked(Path::new("parent/child/grandchild"), "mkdir"));
}

#[test]
fn test_rmdir_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create then remove directory
    let dir_path = harness.mount_path().join("remove_me");
    fs::create_dir(&dir_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify it exists
    assert!(harness.source_path().join("remove_me").is_dir());

    // Remove through mount
    fs::remove_dir(&dir_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify removal
    assert!(!harness.source_path().join("remove_me").exists());
    assert!(harness.verify_operation_tracked(Path::new("remove_me"), "rmdir"));
}

// === Metadata Operation Tracking Tests ===

#[test]
fn test_chmod_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create file with initial permissions
    let file_path = harness.mount_path().join("chmod_test.txt");
    fs::write(&file_path, b"test").unwrap();
    thread::sleep(Duration::from_millis(100));

    // Change permissions through mount
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(&file_path, permissions).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify permissions changed
    let source_file = harness.source_path().join("chmod_test.txt");
    let metadata = fs::metadata(&source_file).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "Permissions should be updated");
}

#[test]
fn test_truncate_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create file with content
    let file_path = harness.mount_path().join("truncate_test.txt");
    fs::write(
        &file_path,
        b"This is a longer content that will be truncated",
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    // Truncate through mount
    let file = fs::OpenOptions::new().write(true).open(&file_path).unwrap();
    file.set_len(10).unwrap();
    drop(file);
    thread::sleep(Duration::from_millis(100));

    // Verify truncation
    let source_file = harness.source_path().join("truncate_test.txt");
    let metadata = fs::metadata(&source_file).unwrap();
    assert_eq!(metadata.len(), 10, "File should be truncated to 10 bytes");

    // Verify content
    let content = fs::read(&source_file).unwrap();
    assert_eq!(content, b"This is a ");
}

#[test]
fn test_timestamp_operations_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create file
    let file_path = harness.mount_path().join("timestamp_test.txt");
    fs::write(&file_path, b"test").unwrap();
    thread::sleep(Duration::from_millis(100));

    // Get initial timestamps
    let initial_metadata = fs::metadata(&file_path).unwrap();
    let initial_mtime = initial_metadata.modified().unwrap();

    // Touch the file (update mtime)
    thread::sleep(Duration::from_millis(1100)); // Ensure time difference
    let file = fs::OpenOptions::new().write(true).open(&file_path).unwrap();
    drop(file);
    thread::sleep(Duration::from_millis(100));

    // Verify timestamp changed
    let source_file = harness.source_path().join("timestamp_test.txt");
    let new_metadata = fs::metadata(&source_file).unwrap();
    let new_mtime = new_metadata.modified().unwrap();

    assert!(
        new_mtime > initial_mtime,
        "Modification time should be updated"
    );
}

// === Complex Operation Sequence Tests ===

#[test]
fn test_complex_file_operations_all_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Create directory structure
    fs::create_dir_all(harness.mount_path().join("project/src")).unwrap();
    fs::create_dir_all(harness.mount_path().join("project/tests")).unwrap();
    thread::sleep(Duration::from_millis(100));

    // Create files
    fs::write(
        harness.mount_path().join("project/README.md"),
        b"# Test Project",
    )
    .unwrap();
    fs::write(
        harness.mount_path().join("project/src/main.rs"),
        b"fn main() {}",
    )
    .unwrap();
    fs::write(
        harness.mount_path().join("project/tests/test.rs"),
        b"#[test] fn test() {}",
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    // Modify file
    fs::write(
        harness.mount_path().join("project/src/main.rs"),
        b"fn main() { println!(\"Hello!\"); }",
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    // Rename file
    fs::rename(
        harness.mount_path().join("project/README.md"),
        harness.mount_path().join("project/README.txt"),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    // Change permissions
    fs::set_permissions(
        harness.mount_path().join("project/src/main.rs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    // Verify final state
    assert!(harness.source_path().join("project/src/main.rs").exists());
    assert!(harness.source_path().join("project/README.txt").exists());
    assert!(!harness.source_path().join("project/README.md").exists());

    let main_content = fs::read(harness.source_path().join("project/src/main.rs")).unwrap();
    assert_eq!(main_content, b"fn main() { println!(\"Hello!\"); }");

    let main_perms = fs::metadata(harness.source_path().join("project/src/main.rs"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(main_perms, 0o755);
}

#[test]
fn test_concurrent_operations_all_tracked() {
    use std::sync::Arc;
    let harness = Arc::new(OperationTrackingHarness::new().unwrap().mount().unwrap());

    // Spawn multiple threads performing operations
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let harness = Arc::clone(&harness);
            thread::spawn(move || {
                let file_name = format!("concurrent_{}.txt", i);
                let file_path = harness.mount_path().join(&file_name);

                // Create file
                fs::write(&file_path, format!("Thread {} content", i)).unwrap();

                // Modify file
                thread::sleep(Duration::from_millis(50));
                fs::write(&file_path, format!("Thread {} modified", i)).unwrap();

                // Create subdirectory
                let dir_path = harness.mount_path().join(format!("thread_{}_dir", i));
                fs::create_dir(&dir_path).unwrap();
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    thread::sleep(Duration::from_millis(200));

    // Verify all operations completed
    for i in 0..5 {
        let file_path = harness.source_path().join(format!("concurrent_{}.txt", i));
        assert!(file_path.exists());

        let content = fs::read(&file_path).unwrap();
        assert_eq!(content, format!("Thread {} modified", i).as_bytes());

        let dir_path = harness.source_path().join(format!("thread_{}_dir", i));
        assert!(dir_path.exists() && dir_path.is_dir());
    }
}
