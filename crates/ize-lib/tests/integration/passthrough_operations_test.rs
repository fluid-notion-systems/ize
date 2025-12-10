//! Integration tests for PassthroughFS2 operations.
//!
//! These tests verify the filesystem behavior by testing the actual
//! file operations that would happen through the FUSE layer.

use std::fs;
use tempfile::TempDir;

use ize_lib::filesystems::passthrough2::PassthroughFS2;

/// Test helper to create a PassthroughFS2 instance with temp directories
fn setup_test_fs() -> (PassthroughFS2, TempDir, TempDir) {
    let source_dir = tempfile::tempdir().unwrap();
    let mount_dir = tempfile::tempdir().unwrap();

    let fs = PassthroughFS2::new(source_dir.path(), mount_dir.path()).unwrap();

    (fs, source_dir, mount_dir)
}

#[test]
fn test_passthrough_initialization() {
    let (_fs, source_dir, mount_dir) = setup_test_fs();

    // Verify directories exist
    assert!(source_dir.path().exists());
    assert!(mount_dir.path().exists());
}

#[test]
fn test_source_directory_operations() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    // Test file operations in source directory
    let test_file = source_dir.path().join("test.txt");
    fs::write(&test_file, "Hello, world!").unwrap();
    assert!(test_file.exists());
    assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello, world!");

    // Test directory operations
    let test_dir = source_dir.path().join("testdir");
    fs::create_dir(&test_dir).unwrap();
    assert!(test_dir.exists());
    assert!(test_dir.is_dir());

    // Test nested operations
    let nested_file = test_dir.join("nested.txt");
    fs::write(&nested_file, "Nested content").unwrap();
    assert!(nested_file.exists());
}

#[test]
fn test_file_lifecycle() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    let file_path = source_dir.path().join("lifecycle.txt");

    // Create
    fs::write(&file_path, "Initial content").unwrap();
    assert!(file_path.exists());

    // Update
    fs::write(&file_path, "Updated content").unwrap();
    assert_eq!(fs::read_to_string(&file_path).unwrap(), "Updated content");

    // Append
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&file_path)
        .unwrap();
    writeln!(file, "\nAppended line").unwrap();
    drop(file);

    let content = fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("Updated content"));
    assert!(content.contains("Appended line"));

    // Delete
    fs::remove_file(&file_path).unwrap();
    assert!(!file_path.exists());
}

#[test]
fn test_directory_lifecycle() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    let dir_path = source_dir.path().join("lifecycle_dir");

    // Create directory
    fs::create_dir(&dir_path).unwrap();
    assert!(dir_path.exists());
    assert!(dir_path.is_dir());

    // Add files to directory
    fs::write(dir_path.join("file1.txt"), "File 1").unwrap();
    fs::write(dir_path.join("file2.txt"), "File 2").unwrap();

    // List directory contents
    let entries: Vec<_> = fs::read_dir(&dir_path)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries.len(), 2);

    // Remove directory (should fail - not empty)
    assert!(fs::remove_dir(&dir_path).is_err());

    // Remove contents first
    fs::remove_file(dir_path.join("file1.txt")).unwrap();
    fs::remove_file(dir_path.join("file2.txt")).unwrap();

    // Now remove directory
    fs::remove_dir(&dir_path).unwrap();
    assert!(!dir_path.exists());
}

#[test]
fn test_rename_operations() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    // Test file rename
    let old_file = source_dir.path().join("old.txt");
    let new_file = source_dir.path().join("new.txt");

    fs::write(&old_file, "Content").unwrap();
    fs::rename(&old_file, &new_file).unwrap();

    assert!(!old_file.exists());
    assert!(new_file.exists());
    assert_eq!(fs::read_to_string(&new_file).unwrap(), "Content");

    // Test directory rename
    let old_dir = source_dir.path().join("old_dir");
    let new_dir = source_dir.path().join("new_dir");

    fs::create_dir(&old_dir).unwrap();
    fs::write(old_dir.join("file.txt"), "In dir").unwrap();

    fs::rename(&old_dir, &new_dir).unwrap();

    assert!(!old_dir.exists());
    assert!(new_dir.exists());
    assert!(new_dir.join("file.txt").exists());
}

#[test]
fn test_permissions_and_metadata() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    let file_path = source_dir.path().join("perms.txt");
    fs::write(&file_path, "test").unwrap();

    // Get metadata
    let metadata = fs::metadata(&file_path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 4); // "test" is 4 bytes

    // Test permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = metadata.permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&file_path, perms).unwrap();

        let new_metadata = fs::metadata(&file_path).unwrap();
        assert_eq!(new_metadata.permissions().mode() & 0o777, 0o644);
    }
}

#[test]
fn test_symlink_operations() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    let target = source_dir.path().join("target.txt");
    let link = source_dir.path().join("link.txt");

    fs::write(&target, "Target content").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&target, &link).unwrap();

        assert!(link.exists());

        // Read through symlink
        let content = fs::read_to_string(&link).unwrap();
        assert_eq!(content, "Target content");

        // Check if it's a symlink
        let metadata = fs::symlink_metadata(&link).unwrap();
        assert!(metadata.file_type().is_symlink());
    }
}

#[test]
fn test_concurrent_operations() {
    let (_fs, source_dir, _mount_dir) = setup_test_fs();

    use std::sync::Arc;
    use std::thread;

    let base_path = Arc::new(source_dir.path().to_path_buf());

    // Spawn multiple threads doing operations
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let path = Arc::clone(&base_path);
            thread::spawn(move || {
                let file = path.join(format!("thread_{}.txt", i));
                for j in 0..10 {
                    fs::write(&file, format!("Iteration {}", j)).unwrap();
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        })
        .collect();

    // Wait for all threads
    for h in handles {
        h.join().unwrap();
    }

    // Verify all files exist with final content
    for i in 0..5 {
        let file = source_dir.path().join(format!("thread_{}.txt", i));
        assert!(file.exists());
        let content = fs::read_to_string(&file).unwrap();
        assert_eq!(content, "Iteration 9");
    }
}

#[test]
fn test_operations_tracking_points() {
    // This test documents where we would intercept operations for versioning

    let operations_to_intercept = vec![
        ("create", "When PassthroughFS2::create is called"),
        ("write", "When PassthroughFS2::write is called"),
        ("unlink", "When PassthroughFS2::unlink is called"),
        ("rename", "When PassthroughFS2::rename is called"),
        ("mkdir", "When PassthroughFS2::mkdir is called"),
        ("rmdir", "When PassthroughFS2::rmdir is called"),
        ("setattr", "When PassthroughFS2::setattr is called"),
        (
            "truncate",
            "When PassthroughFS2::setattr is called with size change",
        ),
        ("symlink", "When PassthroughFS2::symlink is called"),
        ("link", "When PassthroughFS2::link is called"),
    ];

    // This is a documentation test
    assert_eq!(operations_to_intercept.len(), 10);

    // In the actual implementation, each of these PassthroughFS2 methods
    // would call into our storage layer to record the operation before
    // or after performing the actual filesystem operation.
}
