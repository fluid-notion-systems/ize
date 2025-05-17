//! Test for the timestamp functionality in PassthroughFS.

use serial_test::serial;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

use claris_fuse_lib::filesystems::passthrough::PassthroughFS;

/// A test harness for the timestamp functionality
struct TimestampTest {
    source_dir: tempfile::TempDir,
    mount_dir: tempfile::TempDir,
    _db_path: PathBuf,
    mount_thread: Option<thread::JoinHandle<()>>,
    is_mounted: bool,
}

impl TimestampTest {
    /// Set up the test environment with mounted filesystem
    fn setup() -> io::Result<Self> {
        // Create temporary directories
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;

        // Create a database file
        let db_path = source_dir.path().join("fs.db");
        fs::write(&db_path, "dummy content")?;

        // Create the filesystem
        let fs = PassthroughFS::new(&db_path, mount_dir.path())?;

        // Mount the filesystem in a separate thread
        let _mount_point = mount_dir.path().to_path_buf();
        let mount_thread = thread::spawn(move || {
            if let Err(e) = fs.mount() {
                eprintln!("Mount error: {}", e);
            }
        });

        // Give the filesystem time to mount
        thread::sleep(Duration::from_millis(1000));

        Ok(TimestampTest {
            source_dir,
            mount_dir,
            _db_path: db_path,
            mount_thread: Some(mount_thread),
            is_mounted: true,
        })
    }

    fn create_file(&self, name: &str, content: &str) -> io::Result<PathBuf> {
        let file_path = self.mount_dir.path().join(name);
        fs::write(&file_path, content)?;
        Ok(file_path)
    }

    fn get_source_path(&self, name: &str) -> PathBuf {
        self.source_dir.path().join(name)
    }
}

impl Drop for TimestampTest {
    fn drop(&mut self) {
        if self.is_mounted {
            // Use fusermount to unmount
            let _ = Command::new("fusermount")
                .arg("-u")
                .arg(self.mount_dir.path())
                .status();

            // Wait for the mount thread to finish
            if let Some(handle) = self.mount_thread.take() {
                let _ = handle.join();
            }

            self.is_mounted = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial]
    fn test_touch_command() -> io::Result<()> {
        // Set up test environment
        let test = TimestampTest::setup()?;

        // Create a test file
        let file_path = test.create_file("test_touch.txt", "Hello, world!")?;

        // Get initial modification time
        let initial_metadata = fs::metadata(&file_path)?;
        let initial_mtime = initial_metadata.modified()?;

        // Wait a bit to ensure timestamps can change
        thread::sleep(Duration::from_secs(2));

        // Run touch command on the file
        let status = Command::new("touch").arg(&file_path).status()?;

        assert!(status.success(), "Touch command failed");

        // Get updated metadata
        let updated_metadata = fs::metadata(&file_path)?;
        let updated_mtime = updated_metadata.modified()?;

        // Verify the timestamp was updated
        assert!(
            updated_mtime > initial_mtime,
            "File modification time should have been updated. Initial: {:?}, Updated: {:?}",
            initial_mtime,
            updated_mtime
        );

        // Also check the source file's timestamp
        let source_path = test.get_source_path("test_touch.txt");
        let source_metadata = fs::metadata(&source_path)?;
        let source_mtime = source_metadata.modified()?;

        // Verify the source timestamp also changed
        assert!(
            source_mtime > initial_mtime,
            "Source file modification time should have been updated. Initial: {:?}, Source: {:?}",
            initial_mtime,
            source_mtime
        );

        Ok(())
    }
}
