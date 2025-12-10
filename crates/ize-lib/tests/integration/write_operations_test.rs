use fuser::MountOption;
use ize_lib::filesystems::passthrough::PassthroughFS;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::Duration;
use tempfile::{tempdir, TempDir};

/// Test harness for mounting and testing the filesystem
struct FilesystemMountHarness {
    source_dir: TempDir,
    mount_dir: TempDir,
    _session: Option<fuser::BackgroundSession>,
}

impl FilesystemMountHarness {
    /// Create a new harness with directories but no mount
    fn new() -> io::Result<Self> {
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;

        Ok(Self {
            source_dir,
            mount_dir,
            _session: None,
        })
    }

    /// Mount the filesystem and return self for chaining
    fn with_mount(mut self) -> io::Result<Self> {
        let fs = PassthroughFS::new(self.source_dir.path(), self.mount_dir.path())?;

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
        };
        test_fn(&ctx)
    }

    /// Get list of "dirty" files (files that exist in source)
    fn get_dirty_files(&self) -> io::Result<Vec<String>> {
        let mut dirty = Vec::new();

        fn check_dir(path: &Path, base: &Path, dirty: &mut Vec<String>) -> io::Result<()> {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let file_type = entry.file_type()?;

                let rel_path = entry
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();

                if file_type.is_file() {
                    dirty.push(rel_path);
                } else if file_type.is_dir() {
                    dirty.push(rel_path.clone() + "/");
                    check_dir(&entry.path(), base, dirty)?;
                }
            }
            Ok(())
        }

        check_dir(self.source_dir.path(), self.source_dir.path(), &mut dirty)?;
        Ok(dirty)
    }
}

impl Drop for FilesystemMountHarness {
    fn drop(&mut self) {
        // Session will be dropped automatically, which unmounts
    }
}

struct WriteOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
}

impl<'a> WriteOperationsContext<'a> {
    fn write_file(&self, name: &str, content: &[u8]) -> io::Result<()> {
        fs::write(self.mount_path.join(name), content)
    }

    fn append_file(&self, name: &str, content: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.mount_path.join(name))?;
        file.write_all(content)
    }

    fn write_large_file(&self, name: &str, size: usize) -> io::Result<()> {
        let content: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        fs::write(self.mount_path.join(name), content)
    }

    fn verify_in_source(&self, name: &str, expected: &[u8]) -> bool {
        match fs::read(self.source_path.join(name)) {
            Ok(content) => content == expected,
            Err(_) => false,
        }
    }

    fn verify_in_mount(&self, name: &str, expected: &[u8]) -> bool {
        match fs::read(self.mount_path.join(name)) {
            Ok(content) => content == expected,
            Err(_) => false,
        }
    }
}

struct DirectoryOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
}

impl<'a> DirectoryOperationsContext<'a> {
    fn create_dir(&self, name: &str) -> io::Result<()> {
        fs::create_dir(self.mount_path.join(name))
    }

    fn create_dir_all(&self, name: &str) -> io::Result<()> {
        fs::create_dir_all(self.mount_path.join(name))
    }

    fn create_populated_dir(&self, name: &str, file_count: usize) -> io::Result<()> {
        let dir_path = self.mount_path.join(name);
        fs::create_dir(&dir_path)?;

        for i in 0..file_count {
            fs::write(
                dir_path.join(format!("file_{}.txt", i)),
                format!("Content {}", i),
            )?;
        }
        Ok(())
    }

    fn verify_dir_exists_in_source(&self, name: &str) -> bool {
        let path = self.source_path.join(name);
        path.exists() && path.is_dir()
    }

    fn verify_dir_contents(&self, name: &str, expected_count: usize) -> bool {
        match fs::read_dir(self.source_path.join(name)) {
            Ok(entries) => entries.count() == expected_count,
            Err(_) => false,
        }
    }
}

struct MetadataOperationsContext<'a> {
    source_path: &'a Path,
    mount_path: &'a Path,
}

impl<'a> MetadataOperationsContext<'a> {
    fn set_permissions(&self, name: &str, mode: u32) -> io::Result<()> {
        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(self.mount_path.join(name), perms)
    }

    fn truncate_file(&self, name: &str, size: u64) -> io::Result<()> {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(self.mount_path.join(name))?;
        file.set_len(size)
    }

    fn verify_permissions(&self, name: &str, expected_mode: u32) -> bool {
        match fs::metadata(self.source_path.join(name)) {
            Ok(meta) => {
                let mode = meta.permissions().mode() & 0o777;
                mode == expected_mode
            }
            Err(_) => false,
        }
    }

    fn verify_size(&self, name: &str, expected_size: u64) -> bool {
        match fs::metadata(self.source_path.join(name)) {
            Ok(meta) => meta.len() == expected_size,
            Err(_) => false,
        }
    }
}

// === Tests without mount (harness creation) ===

#[test]
fn test_harness_creation_without_mount() -> io::Result<()> {
    let harness = FilesystemMountHarness::new()?;

    // Verify directories exist
    assert!(harness.source_dir.path().exists());
    assert!(harness.mount_dir.path().exists());

    // No dirty files initially
    let dirty = harness.get_dirty_files()?;
    assert!(dirty.is_empty());

    Ok(())
}

// === Write Operations Tests ===

#[test]
fn test_simple_file_write_creates_dirty_entry() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_write_operations(|ctx| {
        ctx.write_file("test.txt", b"Hello, world!").unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_in_source("test.txt", b"Hello, world!"));
        assert!(ctx.verify_in_mount("test.txt", b"Hello, world!"));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"test.txt".to_string()));

    Ok(())
}

#[test]
fn test_multiple_file_writes_track_all_dirty() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_write_operations(|ctx| {
        ctx.write_file("file1.txt", b"Content 1").unwrap();
        ctx.write_file("file2.txt", b"Content 2").unwrap();
        ctx.write_file("file3.txt", b"Content 3").unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_in_source("file1.txt", b"Content 1"));
        assert!(ctx.verify_in_source("file2.txt", b"Content 2"));
        assert!(ctx.verify_in_source("file3.txt", b"Content 3"));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"file1.txt".to_string()));
    assert!(dirty.contains(&"file2.txt".to_string()));
    assert!(dirty.contains(&"file3.txt".to_string()));

    Ok(())
}

#[test]
fn test_file_append_marks_as_dirty() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_write_operations(|ctx| {
        ctx.write_file("append.txt", b"Initial").unwrap();
        thread::sleep(Duration::from_millis(50));

        ctx.append_file("append.txt", b" Appended").unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_in_source("append.txt", b"Initial Appended"));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"append.txt".to_string()));

    Ok(())
}

#[test]
fn test_large_file_write_handles_correctly() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_write_operations(|ctx| {
        ctx.write_large_file("large.bin", 1024 * 1024).unwrap(); // 1MB
        thread::sleep(Duration::from_millis(200));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"large.bin".to_string()));

    Ok(())
}

// === Directory Operations Tests ===

#[test]
fn test_create_directory_marks_as_dirty() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_directory_operations(|ctx| {
        ctx.create_dir("newdir").unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_dir_exists_in_source("newdir"));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"newdir/".to_string()));

    Ok(())
}

#[test]
fn test_nested_directory_creation_tracks_all() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_directory_operations(|ctx| {
        ctx.create_dir_all("parent/child/grandchild").unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_dir_exists_in_source("parent"));
        assert!(ctx.verify_dir_exists_in_source("parent/child"));
        assert!(ctx.verify_dir_exists_in_source("parent/child/grandchild"));
    });

    Ok(())
}

#[test]
fn test_directory_with_files_tracks_correctly() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_directory_operations(|ctx| {
        ctx.create_populated_dir("project", 3).unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_dir_exists_in_source("project"));
        assert!(ctx.verify_dir_contents("project", 3));
    });

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"project/".to_string()));

    Ok(())
}

// === Metadata Operations Tests ===

#[test]
fn test_permission_change_marks_as_dirty() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_metadata_operations(|ctx| {
        fs::write(ctx.mount_path.join("perms.txt"), b"test").unwrap();
        thread::sleep(Duration::from_millis(50));

        ctx.set_permissions("perms.txt", 0o600).unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_permissions("perms.txt", 0o600));
    });

    Ok(())
}

#[test]
fn test_file_truncate_marks_as_dirty() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_metadata_operations(|ctx| {
        fs::write(
            ctx.mount_path.join("truncate.txt"),
            b"This is some longer content",
        )
        .unwrap();
        thread::sleep(Duration::from_millis(50));

        ctx.truncate_file("truncate.txt", 10).unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_size("truncate.txt", 10));
    });

    Ok(())
}

#[test]
fn test_metadata_only_changes_track_correctly() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    harness.test_metadata_operations(|ctx| {
        fs::write(ctx.mount_path.join("meta.txt"), b"content").unwrap();
        thread::sleep(Duration::from_millis(50));

        // Change permissions
        ctx.set_permissions("meta.txt", 0o755).unwrap();
        thread::sleep(Duration::from_millis(100));

        assert!(ctx.verify_permissions("meta.txt", 0o755));
    });

    Ok(())
}

// === Mixed Operations Tests ===

#[test]
fn test_mixed_operations_all_tracked() -> io::Result<()> {
    let mut harness = FilesystemMountHarness::new()?.with_mount()?;

    // Write operations
    harness.test_write_operations(|ctx| {
        ctx.write_file("file.txt", b"content").unwrap();
    });

    // Directory operations
    harness.test_directory_operations(|ctx| {
        ctx.create_dir("dir").unwrap();
    });

    // Metadata operations
    harness.test_metadata_operations(|ctx| {
        ctx.set_permissions("file.txt", 0o644).unwrap();
    });

    thread::sleep(Duration::from_millis(100));

    let dirty = harness.get_dirty_files()?;
    assert!(dirty.contains(&"file.txt".to_string()));
    assert!(dirty.contains(&"dir/".to_string()));

    Ok(())
}
