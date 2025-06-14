//! Integration tests for PassthroughFS with actual FUSE mounting.
//!
//! These tests mount the filesystem and perform real operations through
//! the FUSE layer to verify the complete system behavior.

use serial_test::serial;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

use claris_fuse_lib::filesystems::passthrough::PassthroughFS;

/// Test harness for mounted filesystem tests
struct MountedTest {
    source_dir: TempDir,
    mount_dir: TempDir,
    db_path: PathBuf,
    _mount_handle: thread::JoinHandle<()>,
}

impl MountedTest {
    /// Create and mount a test filesystem
    fn new() -> io::Result<Self> {
        let source_dir = tempfile::tempdir()?;
        let mount_dir = tempfile::tempdir()?;
        let db_path = source_dir.path().join("test.db");

        // Create the database file
        fs::write(&db_path, "test")?;

        // Create the filesystem
        let fs = PassthroughFS::new(&db_path, mount_dir.path())?;

        // Mount in a background thread
        let mount_handle = thread::spawn(move || {
            if let Err(e) = fs.mount() {
                eprintln!("Mount error: {}", e);
            }
        });

        // Wait for mount to complete
        thread::sleep(Duration::from_millis(500));

        // Verify mount succeeded
        if !is_mounted(mount_dir.path()) {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Failed to mount filesystem",
            ));
        }

        Ok(Self {
            source_dir,
            mount_dir,
            db_path,
            _mount_handle: mount_handle,
        })
    }

    /// Get the mount path
    fn mount_path(&self) -> &Path {
        self.mount_dir.path()
    }

    /// Get the source path
    fn source_path(&self) -> &Path {
        self.source_dir.path()
    }
}

impl Drop for MountedTest {
    fn drop(&mut self) {
        // Unmount the filesystem
        if let Err(e) = unmount(self.mount_dir.path()) {
            eprintln!("Failed to unmount: {}", e);
        }
        // The mount thread will exit when unmounted
    }
}

/// Check if a path is mounted
fn is_mounted(path: &Path) -> bool {
    match fs::read_dir(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Unmount a filesystem
fn unmount(path: &Path) -> io::Result<()> {
    let output = Command::new("fusermount").arg("-u").arg(path).output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "fusermount failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }

    Ok(())
}

#[test]
#[serial]
fn test_mount_and_unmount() -> io::Result<()> {
    let test = MountedTest::new()?;

    // Verify we can list the mount directory
    let entries: Vec<_> = fs::read_dir(test.mount_path())?.collect();
    assert_eq!(entries.len(), 0, "Mount should be empty initially");

    Ok(())
}

#[test]
#[serial]
fn test_file_operations_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    // Create a file through the mount
    let mount_file = test.mount_path().join("test.txt");
    fs::write(&mount_file, "Hello, FUSE!")?;

    // Verify it exists in the mount
    assert!(mount_file.exists());
    let content = fs::read_to_string(&mount_file)?;
    assert_eq!(content, "Hello, FUSE!");

    // Verify it exists in the source
    let source_file = test.source_path().join("test.txt");
    assert!(source_file.exists());
    let source_content = fs::read_to_string(&source_file)?;
    assert_eq!(source_content, "Hello, FUSE!");

    Ok(())
}

#[test]
#[serial]
fn test_directory_operations_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    // Create directory through mount
    let mount_dir = test.mount_path().join("testdir");
    fs::create_dir(&mount_dir)?;

    // Create file in directory
    let mount_file = mount_dir.join("file.txt");
    fs::write(&mount_file, "In directory")?;

    // Verify in mount
    assert!(mount_dir.exists());
    assert!(mount_file.exists());

    // Verify in source
    let source_dir = test.source_path().join("testdir");
    let source_file = source_dir.join("file.txt");
    assert!(source_dir.exists());
    assert!(source_file.exists());

    Ok(())
}

#[test]
#[serial]
fn test_rename_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    // Create file
    let original = test.mount_path().join("original.txt");
    fs::write(&original, "To be renamed")?;

    // Rename it
    let renamed = test.mount_path().join("renamed.txt");
    fs::rename(&original, &renamed)?;

    // Verify in mount
    assert!(!original.exists());
    assert!(renamed.exists());

    // Verify in source
    let source_original = test.source_path().join("original.txt");
    let source_renamed = test.source_path().join("renamed.txt");
    assert!(!source_original.exists());
    assert!(source_renamed.exists());

    Ok(())
}

#[test]
#[serial]
fn test_delete_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    // Create and delete file
    let file = test.mount_path().join("delete_me.txt");
    fs::write(&file, "Temporary")?;
    assert!(file.exists());

    fs::remove_file(&file)?;
    assert!(!file.exists());

    // Verify in source
    let source_file = test.source_path().join("delete_me.txt");
    assert!(!source_file.exists());

    Ok(())
}

#[test]
#[serial]
fn test_append_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    let file = test.mount_path().join("append.txt");

    // Write initial content
    fs::write(&file, "Line 1\n")?;

    // Append more content
    use std::fs::OpenOptions;
    use std::io::Write;

    let mut f = OpenOptions::new().append(true).open(&file)?;
    writeln!(f, "Line 2")?;
    writeln!(f, "Line 3")?;
    drop(f);

    // Verify full content
    let content = fs::read_to_string(&file)?;
    assert_eq!(content, "Line 1\nLine 2\nLine 3\n");

    // Verify in source
    let source_file = test.source_path().join("append.txt");
    let source_content = fs::read_to_string(&source_file)?;
    assert_eq!(source_content, "Line 1\nLine 2\nLine 3\n");

    Ok(())
}

#[test]
#[serial]
fn test_permissions_through_mount() -> io::Result<()> {
    let test = MountedTest::new()?;

    let file = test.mount_path().join("perms.txt");
    fs::write(&file, "test")?;

    // Set permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&file)?.permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&file, perms)?;

        // Verify permissions
        let meta = fs::metadata(&file)?;
        assert_eq!(meta.permissions().mode() & 0o777, 0o644);
    }

    Ok(())
}

#[test]
#[serial]
#[ignore = "Requires careful setup to avoid interference"]
fn test_concurrent_operations() -> io::Result<()> {
    let test = MountedTest::new()?;

    use std::sync::Arc;
    let mount_path = Arc::new(test.mount_path().to_path_buf());

    // Spawn multiple threads doing operations
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let path = Arc::clone(&mount_path);
            thread::spawn(move || {
                let file = path.join(format!("thread_{}.txt", i));
                for j in 0..10 {
                    fs::write(&file, format!("Iteration {}", j)).unwrap();
                }
            })
        })
        .collect();

    // Wait for all threads
    for h in handles {
        h.join().unwrap();
    }

    // Verify all files exist
    for i in 0..5 {
        let file = test.mount_path().join(format!("thread_{}.txt", i));
        assert!(file.exists());
    }

    Ok(())
}
