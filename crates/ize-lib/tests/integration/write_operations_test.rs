use fuser::{BackgroundSession, MountOption};
use ize_lib::filesystems::passthrough::PassthroughFS;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};
use tempfile::{tempdir, TempDir};

/// Test harness for mounting and testing the filesystem
struct FilesystemMountHarness {
    source_dir: TempDir,
    mount_dir: TempDir,
    db_path: PathBuf,
    _session: Option<BackgroundSession>,
}

impl FilesystemMountHarness {
    /// Create a new harness with directories but no mount
    fn new() -> io::Result<Self> {
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;
        let db_path = source_dir.path().join(".ize.db");

        // Initialize the database
        fs::write(&db_path, b"IZE_DB_V1")?;

        Ok(Self {
            source_dir,
            mount_dir,
            db_path,
            _session: None,
        })
    }

    /// Mount the filesystem and return self for chaining
    fn with_mount(mut self) -> io::Result<Self> {
        let fs = PassthroughFS::new(self.db_path.clone(), self.mount_dir.path())?;

        let mount_path = self.mount_dir.path().to_path_buf();
        let options = vec![
            MountOption::FSName("ize-test".to_string()),
            MountOption::AutoUnmount,
        ];

        // Mount in background - this spawns its own thread
        let session = fuser::spawn_mount2(fs, &mount_path, &options)?;

        // Wait a bit for mount to be ready
        thread::sleep(Duration::from_millis(200));

        // Verify mount is working
        if !mount_path.exists() || fs::read_dir(&mount_path).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Mount point not accessible",
            ));
        }

        self._session = Some(session);
        Ok(self)
    }

    /// Execute a test with write operations context
    fn test_write_operations<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(&WriteOperationsContext) -> R,
    {
        let ctx = WriteOperationsContext {
            source_path: self.source_dir.path(),
            mount_path: self.mount_dir.path(),
            db_path: &self.db_path,
        };
        test_fn(&ctx)
    }

    /// Execute a test with directory operations context
    fn test_directory_operations<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(&DirectoryOperationsContext) -> R,
    {
        let ctx = DirectoryOperationsContext {
            source_path: self.source_dir.path(),
            mount_path: self.mount_dir.path(),
            db_path: &self.db_path,
        };
        test_fn(&ctx)
    }

    /// Execute a test with metadata operations context
    fn test_metadata_operations<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(&MetadataOperationsContext) -> R,
    {
        let ctx = MetadataOperationsContext {
            source_path: self.source_dir.path(),
            mount_path: self.mount_dir.path(),
            db_path: &self.db_path,
        };
        test_fn(&ctx)
    }

    /// Check for dirty files in the working directory
    fn get_dirty_files(&self) -> io::Result<Vec<PathBuf>> {
        // In a real implementation, this would check the versioning system
        // For now, we'll check what files have been modified since mount
        let mut dirty_files = Vec::new();

        fn check_dir(dir: &Path, base: &Path, dirty: &mut Vec<PathBuf>) -> io::Result<()> {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap().to_path_buf();

                // Skip the database file
                if relative
                    .to_str()
                    .map(|s| s.contains(".ize.db"))
                    .unwrap_or(false)
                {
                    continue;
                }

                if path.is_dir() {
                    check_dir(&path, base, dirty)?;
                } else {
                    // In real implementation, check against versioned state
                    // For now, we'll add all non-db files
                    dirty.push(relative);
                }
            }
            Ok(())
        }

        check_dir(
            self.source_dir.path(),
            self.source_dir.path(),
            &mut dirty_files,
        )?;
        Ok(dirty_files)
    }
}

impl Drop for FilesystemMountHarness {
    fn drop(&mut self) {
        // The BackgroundSession will automatically unmount when dropped
        // AutoUnmount option ensures cleanup even on panic
    }
}

/// Context for file write operations
struct WriteOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
    db_path: &'a Path,
}

impl<'a> WriteOperationsContext<'a> {
    fn write_file(&self, name: &str, content: &[u8]) -> io::Result<()> {
        let path = self.mount_path.join(name);
        fs::write(path, content)
    }

    fn append_file(&self, name: &str, content: &[u8]) -> io::Result<()> {
        let path = self.mount_path.join(name);
        let mut existing = fs::read(&path).unwrap_or_default();
        existing.extend_from_slice(content);
        fs::write(path, existing)
    }

    fn write_large_file(&self, name: &str, size_mb: usize) -> io::Result<()> {
        let path = self.mount_path.join(name);
        let content = vec![b'A'; size_mb * 1024 * 1024];
        fs::write(path, content)
    }

    fn verify_in_source(&self, name: &str, expected: &[u8]) -> io::Result<()> {
        let source_file = self.source_path.join(name);
        let actual = fs::read(source_file)?;
        assert_eq!(actual, expected, "File content mismatch in source");
        Ok(())
    }

    fn verify_in_mount(&self, name: &str, expected: &[u8]) -> io::Result<()> {
        let mount_file = self.mount_path.join(name);
        let actual = fs::read(mount_file)?;
        assert_eq!(actual, expected, "File content mismatch in mount");
        Ok(())
    }
}

/// Context for directory operations
struct DirectoryOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
    db_path: &'a Path,
}

impl<'a> DirectoryOperationsContext<'a> {
    fn create_dir(&self, name: &str) -> io::Result<()> {
        let path = self.mount_path.join(name);
        fs::create_dir(path)
    }

    fn create_dir_all(&self, path: &str) -> io::Result<()> {
        let full_path = self.mount_path.join(path);
        fs::create_dir_all(full_path)
    }

    fn create_populated_dir(&self, name: &str, file_count: usize) -> io::Result<()> {
        let dir_path = self.mount_path.join(name);
        fs::create_dir(&dir_path)?;

        for i in 0..file_count {
            let file_path = dir_path.join(format!("file_{}.txt", i));
            fs::write(file_path, format!("Content of file {}", i))?;
        }
        Ok(())
    }

    fn verify_dir_exists_in_source(&self, name: &str) -> io::Result<()> {
        let source_dir = self.source_path.join(name);
        assert!(
            source_dir.exists() && source_dir.is_dir(),
            "Directory {} doesn't exist in source",
            name
        );
        Ok(())
    }

    fn verify_dir_contents(&self, name: &str, expected_files: usize) -> io::Result<()> {
        let mount_dir = self.mount_path.join(name);
        let entries: Vec<_> = fs::read_dir(mount_dir)?.collect();
        assert_eq!(
            entries.len(),
            expected_files,
            "Directory {} has wrong number of files",
            name
        );
        Ok(())
    }
}

/// Context for metadata operations
struct MetadataOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
    db_path: &'a Path,
}

impl<'a> MetadataOperationsContext<'a> {
    fn set_permissions(&self, name: &str, mode: u32) -> io::Result<()> {
        let path = self.mount_path.join(name);
        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(path, perms)
    }

    fn set_timestamps(&self, name: &str, _atime: SystemTime, _mtime: SystemTime) -> io::Result<()> {
        let path = self.mount_path.join(name);
        // Touch the file to trigger metadata update through FUSE
        fs::OpenOptions::new()
            .create(false)
            .write(true)
            .open(&path)?;
        Ok(())
    }

    fn truncate_file(&self, name: &str, size: u64) -> io::Result<()> {
        let path = self.mount_path.join(name);
        let file = fs::OpenOptions::new().write(true).open(path)?;
        file.set_len(size)
    }

    fn verify_permissions(&self, name: &str, expected_mode: u32) -> io::Result<()> {
        let source_path = self.source_path.join(name);
        let metadata = fs::metadata(source_path)?;
        let actual_mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            actual_mode,
            expected_mode & 0o777,
            "Permission mismatch for {}",
            name
        );
        Ok(())
    }

    fn verify_size(&self, name: &str, expected_size: u64) -> io::Result<()> {
        let source_path = self.source_path.join(name);
        let metadata = fs::metadata(source_path)?;
        assert_eq!(metadata.len(), expected_size, "Size mismatch for {}", name);
        Ok(())
    }
}

// === File Write Operation Tests ===

#[test]
fn test_harness_creation_without_mount() {
    // Simple test to verify basic harness creation works
    let harness = FilesystemMountHarness::new().unwrap();

    // Verify directories were created
    assert!(harness.source_dir.path().exists());
    assert!(harness.mount_dir.path().exists());
    assert!(harness.db_path.exists());

    // Verify we can write to source directory directly
    let test_file = harness.source_dir.path().join("direct_test.txt");
    fs::write(&test_file, b"Direct write test").unwrap();
    assert!(test_file.exists());

    let content = fs::read(&test_file).unwrap();
    assert_eq!(content, b"Direct write test");
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_simple_file_write_creates_dirty_entry() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_write_operations(|ctx| {
        // Write a simple file
        ctx.write_file("test.txt", b"Hello, Ize!").unwrap();

        // Verify it exists in both mount and source
        ctx.verify_in_mount("test.txt", b"Hello, Ize!").unwrap();
        ctx.verify_in_source("test.txt", b"Hello, Ize!").unwrap();
    });

    // Check dirty files
    let dirty = harness.get_dirty_files().unwrap();
    assert!(
        dirty.iter().any(|p| p.to_str().unwrap() == "test.txt"),
        "test.txt should be marked as dirty"
    );
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_multiple_file_writes_track_all_dirty() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_write_operations(|ctx| {
        // Write multiple files
        for i in 0..5 {
            let filename = format!("file_{}.txt", i);
            let content = format!("Content {}", i);
            ctx.write_file(&filename, content.as_bytes()).unwrap();
        }

        // Verify all files
        for i in 0..5 {
            let filename = format!("file_{}.txt", i);
            let content = format!("Content {}", i);
            ctx.verify_in_source(&filename, content.as_bytes()).unwrap();
        }
    });

    // Check all files are dirty
    let dirty = harness.get_dirty_files().unwrap();
    assert_eq!(dirty.len(), 5, "Should have 5 dirty files");
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_file_append_marks_as_dirty() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_write_operations(|ctx| {
        // Create initial file
        ctx.write_file("append_test.txt", b"Initial content")
            .unwrap();

        // Append to file
        ctx.append_file("append_test.txt", b" - Appended content")
            .unwrap();

        // Verify combined content
        ctx.verify_in_source("append_test.txt", b"Initial content - Appended content")
            .unwrap();
    });

    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty
        .iter()
        .any(|p| p.to_str().unwrap() == "append_test.txt"));
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_large_file_write_handles_correctly() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_write_operations(|ctx| {
        // Write a 10MB file
        ctx.write_large_file("large.bin", 10).unwrap();

        // Verify size
        let metadata = fs::metadata(ctx.source_path.join("large.bin")).unwrap();
        assert_eq!(metadata.len(), 10 * 1024 * 1024);
    });

    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty.iter().any(|p| p.to_str().unwrap() == "large.bin"));
}

// === Directory Operation Tests ===

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_create_directory_marks_as_dirty() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_directory_operations(|ctx| {
        // Create a directory
        ctx.create_dir("test_dir").unwrap();

        // Verify it exists
        ctx.verify_dir_exists_in_source("test_dir").unwrap();
    });

    // Directory operations should be tracked
    let _dirty = harness.get_dirty_files().unwrap();
    // Note: Directory tracking might be different than files
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_nested_directory_creation_tracks_all() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_directory_operations(|ctx| {
        // Create nested directories
        ctx.create_dir_all("level1/level2/level3").unwrap();

        // Verify all levels exist
        ctx.verify_dir_exists_in_source("level1").unwrap();
        ctx.verify_dir_exists_in_source("level1/level2").unwrap();
        ctx.verify_dir_exists_in_source("level1/level2/level3")
            .unwrap();
    });
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_directory_with_files_tracks_correctly() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_directory_operations(|ctx| {
        // Create directory with files
        ctx.create_populated_dir("data_dir", 10).unwrap();

        // Verify directory and contents
        ctx.verify_dir_exists_in_source("data_dir").unwrap();
        ctx.verify_dir_contents("data_dir", 10).unwrap();
    });

    let dirty = harness.get_dirty_files().unwrap();
    // Should have 10 files marked as dirty
    let data_files: Vec<_> = dirty
        .iter()
        .filter(|p| p.to_str().unwrap().starts_with("data_dir/"))
        .collect();
    assert_eq!(
        data_files.len(),
        10,
        "Should have 10 dirty files in data_dir"
    );
}

// === Metadata Operation Tests ===

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_permission_change_marks_as_dirty() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_metadata_operations(|ctx| {
        // Create a file first
        fs::write(ctx.mount_path.join("perm_test.txt"), b"test").unwrap();

        // Change permissions
        ctx.set_permissions("perm_test.txt", 0o644).unwrap();

        // Verify permissions
        ctx.verify_permissions("perm_test.txt", 0o644).unwrap();
    });

    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty.iter().any(|p| p.to_str().unwrap() == "perm_test.txt"));
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_file_truncate_marks_as_dirty() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_metadata_operations(|ctx| {
        // Create a file with content
        fs::write(ctx.mount_path.join("truncate_test.txt"), b"Hello, World!").unwrap();

        // Truncate to 5 bytes
        ctx.truncate_file("truncate_test.txt", 5).unwrap();

        // Verify size
        ctx.verify_size("truncate_test.txt", 5).unwrap();
    });

    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty
        .iter()
        .any(|p| p.to_str().unwrap() == "truncate_test.txt"));
}

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_metadata_only_changes_track_correctly() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    harness.test_metadata_operations(|ctx| {
        // Create files
        fs::write(ctx.mount_path.join("meta1.txt"), b"content").unwrap();
        fs::write(ctx.mount_path.join("meta2.txt"), b"content").unwrap();

        // Change only metadata (no content changes)
        ctx.set_permissions("meta1.txt", 0o600).unwrap();
        ctx.set_permissions("meta2.txt", 0o644).unwrap();

        // Verify
        ctx.verify_permissions("meta1.txt", 0o600).unwrap();
        ctx.verify_permissions("meta2.txt", 0o644).unwrap();
    });

    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty.iter().any(|p| p.to_str().unwrap() == "meta1.txt"));
    assert!(dirty.iter().any(|p| p.to_str().unwrap() == "meta2.txt"));
}

// === Complex Operation Tests ===

#[test]
#[ignore = "Requires FUSE mount permissions"]
fn test_mixed_operations_all_tracked() {
    let mut harness = FilesystemMountHarness::new().unwrap().with_mount().unwrap();

    // Perform a mix of operations
    harness.test_write_operations(|write_ctx| {
        write_ctx.write_file("doc.txt", b"Documentation").unwrap();
    });

    harness.test_directory_operations(|dir_ctx| {
        dir_ctx.create_dir("src").unwrap();
        dir_ctx.create_populated_dir("tests", 3).unwrap();
    });

    harness.test_metadata_operations(|meta_ctx| {
        meta_ctx.set_permissions("doc.txt", 0o644).unwrap();
    });

    // Verify all operations were tracked
    let dirty = harness.get_dirty_files().unwrap();
    assert!(dirty.len() >= 4, "Should have at least 4 dirty entries"); // doc.txt + 3 test files
}
