//! Integration tests for mounted filesystem operations.
//!
//! These tests mount the PassthroughFS, perform operations in the mounted directory,
//! and verify the results in the source directory.

use env_logger;
use log::{debug, error, info};
use serial_test::serial;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tempfile::tempdir;

use claris_fuse_lib::filesystems::passthrough::PassthroughFS;

/// A test harness for mounted filesystem tests
struct MountedFsTest {
    source_dir: tempfile::TempDir,
    mount_dir: tempfile::TempDir,
    _db_path: PathBuf,
    mount_thread: Option<JoinHandle<()>>,
    is_mounted: Arc<Mutex<bool>>,
}

impl MountedFsTest {
    /// Set up the test environment with mounted filesystem
    fn setup() -> io::Result<Self> {
        // Create temporary directories
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;

        // Create a database file
        let db_path = source_dir.path().join("fs.db");
        fs::write(&db_path, "dummy content")?;

        // Initialize the mounted filesystem
        let is_mounted = Arc::new(Mutex::new(false));
        let is_mounted_clone = is_mounted.clone();

        // Mount the filesystem in a separate thread
        let mount_point = mount_dir.path().to_path_buf();
        println!("Mount point path: {:?}", mount_point);

        // Check if the mount point is accessible
        match fs::metadata(&mount_point) {
            Ok(meta) => println!("Mount point metadata: is_dir={}", meta.is_dir()),
            Err(e) => println!("Error reading mount point metadata: {}", e),
        }

        // Mount the filesystem in a separate thread
        let mount_point = mount_dir.path().to_path_buf();
        debug!(
            "Creating PassthroughFS with db_path: {:?}, mount_point: {:?}",
            db_path, mount_point
        );
        let fs = PassthroughFS::new(&db_path, &mount_point)?;

        let mount_thread = thread::spawn(move || {
            // Set the is_mounted flag to true
            {
                let mut mounted = is_mounted_clone.lock().unwrap();
                *mounted = true;
                debug!("Setting is_mounted to true");
            }

            // This will block until unmounted
            debug!("Calling fs.mount() - this will block until unmounted");
            if let Err(e) = fs.mount() {
                error!("Mount error: {}", e);
            }

            // This code will run after unmounting
            {
                let mut mounted = is_mounted_clone.lock().unwrap();
                *mounted = false;
                debug!("Setting is_mounted to false");
            }
        });

        // Give the filesystem time to mount
        debug!("Sleeping to give filesystem time to mount");
        thread::sleep(Duration::from_millis(500));
        println!("Waiting for filesystem to mount...");
        thread::sleep(Duration::from_millis(1000)); // Increased timeout

        // Verify mount was successful by listing the directory
        println!("Checking if mount was successful...");
        match fs::read_dir(&mount_point) {
            Ok(entries) => {
                println!(
                    "Mount point is readable, contains {} entries",
                    entries.count()
                );
            }
            Err(e) => {
                println!("Error reading mount directory: {}", e);
            }
        }

        // Also check if the mount point exists
        if !Path::new(&mount_point).exists() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Failed to mount filesystem",
            ));
        }

        Ok(MountedFsTest {
            source_dir,
            mount_dir,
            _db_path: db_path,
            mount_thread: Some(mount_thread),
            is_mounted,
        })
    }

    /// Create a file in the mount directory with the given content
    fn create_mount_file(&self, name: &str, content: &str) -> io::Result<PathBuf> {
        let path = self.mount_dir.path().join(name);
        println!("Creating file at mount path: {:?}", path);

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            println!("Creating parent directory: {:?}", parent);
            match fs::create_dir_all(parent) {
                Ok(_) => println!("Successfully created parent directory"),
                Err(e) => println!("Error creating parent directory: {}", e),
            }
        }

        // Try to write the file with error handling
        match fs::write(&path, content) {
            Ok(_) => println!("Successfully wrote file content"),
            Err(e) => {
                println!("Error writing file: {}", e);
                return Err(e);
            }
        }

        // Verify the file was created
        match fs::metadata(&path) {
            Ok(_) => println!("Successfully verified file exists"),
            Err(e) => println!("Error verifying file: {}", e),
        }

        Ok(path)
    }

    /// Create a directory in the mount directory
    fn create_mount_dir(&self, name: &str) -> io::Result<PathBuf> {
        let path = self.mount_dir.path().join(name);
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Get the corresponding path in the source directory for a mount path
    fn get_source_path(&self, mount_relative_path: &str) -> PathBuf {
        self.source_dir.path().join(mount_relative_path)
    }

    /// Check if the filesystem is mounted
    fn is_mounted(&self) -> bool {
        *self.is_mounted.lock().unwrap()
    }
}

impl Drop for MountedFsTest {
    fn drop(&mut self) {
        // Unmount the filesystem if it's still mounted
        if self.is_mounted() {
            debug!(
                "Dropping MountedFsTest, unmounting filesystem at {:?}",
                self.mount_dir.path()
            );
            // Use fusermount to unmount
            let status = Command::new("fusermount")
                .arg("-u")
                .arg(self.mount_dir.path())
                .status();

            match status {
                Ok(s) => debug!("fusermount -u exit status: {}", s),
                Err(e) => error!("Failed to run fusermount: {}", e),
            }

            // Give it a moment to unmount
            debug!("Sleeping to allow unmount to complete");
            thread::sleep(Duration::from_millis(100));

            // Wait for the mount thread to finish if it exists
            if let Some(handle) = self.mount_thread.take() {
                debug!("Joining mount thread");
                let _ = handle.join();
                debug!("Mount thread joined");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Initialize logger for tests
    fn init() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .is_test(true)
            .try_init();
    }

    #[test]
    #[serial]
    fn test_mount_setup() {
        init();
        info!("Starting test_mount_setup");
        let test = MountedFsTest::setup().expect("Failed to set up mounted filesystem");
        assert!(test.is_mounted(), "Filesystem should be mounted");
        assert!(
            test.mount_dir.path().exists(),
            "Mount directory should exist"
        );
    }

    #[test]
    #[serial]
    fn test_create_file_in_mount() -> io::Result<()> {
        init();
        info!("Starting test_create_file_in_mount");
        let test = MountedFsTest::setup()?;

        // Create a file in the mounted directory
        let content = "Hello from mounted filesystem";
        let mount_path = test.create_mount_file("test.txt", content)?;

        // Verify the file exists in the mount directory
        assert!(mount_path.exists(), "File should exist in mount directory");

        // Verify the file exists in the source directory
        let source_path = test.get_source_path("test.txt");
        assert!(
            source_path.exists(),
            "File should exist in source directory"
        );

        // Verify the content matches
        let mut file = fs::File::open(source_path)?;
        let mut source_content = String::new();
        file.read_to_string(&mut source_content)?;

        assert_eq!(source_content, content, "File content should match");

        Ok(())
    }

    #[test]
    #[serial]
    fn test_create_directory_and_files() -> io::Result<()> {
        init();
        info!("Starting test_create_directory_and_files");
        let test = MountedFsTest::setup()?;

        // Create a directory structure with files
        let dir_path = test.create_mount_dir("nested/dir")?;
        test.create_mount_file("nested/dir/file1.txt", "File 1 content")?;
        test.create_mount_file("nested/dir/file2.txt", "File 2 content")?;

        // Verify directory structure in mount
        assert!(dir_path.exists(), "Directory should exist in mount");
        assert!(
            dir_path.join("file1.txt").exists(),
            "File 1 should exist in mount"
        );
        assert!(
            dir_path.join("file2.txt").exists(),
            "File 2 should exist in mount"
        );

        // Verify directory structure in source
        let source_dir = test.get_source_path("nested/dir");
        assert!(source_dir.exists(), "Directory should exist in source");
        assert!(
            source_dir.join("file1.txt").exists(),
            "File 1 should exist in source"
        );
        assert!(
            source_dir.join("file2.txt").exists(),
            "File 2 should exist in source"
        );

        // Verify file contents
        assert_eq!(
            fs::read_to_string(source_dir.join("file1.txt"))?,
            "File 1 content"
        );
        assert_eq!(
            fs::read_to_string(source_dir.join("file2.txt"))?,
            "File 2 content"
        );

        Ok(())
    }

    #[test]
    #[serial]
    fn test_read_write_append() -> io::Result<()> {
        init();
        info!("Starting test_read_write_append");
        let test = MountedFsTest::setup()?;

        // Create initial file
        let file_path = test.create_mount_file("rw_test.txt", "Initial content\n")?;

        // Read the file
        let content = fs::read_to_string(&file_path)?;
        assert_eq!(content, "Initial content\n");

        // Write to the file
        fs::write(&file_path, "New content\n")?;
        assert_eq!(fs::read_to_string(&file_path)?, "New content\n");

        // Append to the file
        let mut file = fs::OpenOptions::new().append(true).open(&file_path)?;

        write!(file, "Appended line\n")?;

        // Verify the content in the source file
        let source_path = test.get_source_path("rw_test.txt");
        assert_eq!(
            fs::read_to_string(source_path)?,
            "New content\nAppended line\n"
        );

        Ok(())
    }

    #[test]
    #[serial]
    fn test_rename_file() -> io::Result<()> {
        init();
        info!("Starting test_rename_file");
        let test = MountedFsTest::setup()?;

        // Create a file
        let original_path = test.create_mount_file("original.txt", "Test content")?;

        // Rename the file
        let new_path = test.mount_dir.path().join("renamed.txt");
        fs::rename(&original_path, &new_path)?;

        // Verify original doesn't exist and new does
        assert!(!original_path.exists(), "Original file should not exist");
        assert!(new_path.exists(), "Renamed file should exist");

        // Verify in source directory
        assert!(!test.get_source_path("original.txt").exists());
        assert!(test.get_source_path("renamed.txt").exists());

        // Verify content preserved
        assert_eq!(fs::read_to_string(&new_path)?, "Test content");

        Ok(())
    }

    #[test]
    #[serial]
    fn test_remove_file_and_directory() -> io::Result<()> {
        init();
        info!("Starting test_remove_file_and_directory");
        let test = MountedFsTest::setup()?;

        // Create directory and files
        test.create_mount_dir("remove_test")?;
        test.create_mount_file("remove_test/file.txt", "File to be removed")?;

        // Verify they exist
        assert!(test.mount_dir.path().join("remove_test").exists());
        assert!(test.mount_dir.path().join("remove_test/file.txt").exists());

        // Remove the file
        fs::remove_file(test.mount_dir.path().join("remove_test/file.txt"))?;

        // Verify file is gone but directory remains
        assert!(!test.mount_dir.path().join("remove_test/file.txt").exists());
        assert!(test.mount_dir.path().join("remove_test").exists());

        // Remove the directory
        fs::remove_dir(test.mount_dir.path().join("remove_test"))?;

        // Verify directory is gone
        assert!(!test.mount_dir.path().join("remove_test").exists());

        // Verify changes reflected in source
        assert!(!test.get_source_path("remove_test/file.txt").exists());
        assert!(!test.get_source_path("remove_test").exists());

        Ok(())
    }
}
