//! Filesystem-specific test harness for Claris-FUSE
//!
//! Provides minimal infrastructure for filesystem testing without duplicating setup code.

use super::harness::{TestHarness, TestHarnessBuilder, TestResources};
use std::io;
use std::path::{Path, PathBuf};

/// Filesystem test harness for testing filesystem operations
pub struct FilesystemTestHarness {
    resources: TestResources,
    source_dir: Option<PathBuf>,
    mount_dir: Option<PathBuf>,
    db_path: Option<PathBuf>,
}

impl FilesystemTestHarness {
    /// Create a new filesystem test harness
    pub fn new() -> Self {
        Self {
            resources: TestResources::new(),
            source_dir: None,
            mount_dir: None,
            db_path: None,
        }
    }
}

impl TestHarness for FilesystemTestHarness {
    type Context<'a> = FilesystemTestContext<'a>;

    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(Self::Context<'_>) -> R,
    {
        let ctx = FilesystemTestContext {
            source_dir: self.source_dir.as_deref(),
            mount_dir: self.mount_dir.as_deref(),
            db_path: self.db_path.as_deref(),
        };
        test_fn(ctx)
    }

    fn setup(&mut self) -> io::Result<()> {
        // Create source directory if not set
        if self.source_dir.is_none() {
            let dir = self.resources.create_temp_dir()?;
            self.source_dir = Some(dir.to_path_buf());
        }

        // Create mount directory if not set
        if self.mount_dir.is_none() {
            let dir = self.resources.create_temp_dir()?;
            self.mount_dir = Some(dir.to_path_buf());
        }

        // Create DB file if not set
        if self.db_path.is_none() {
            let db_path = self.source_dir.as_ref().unwrap().join("test.db");
            std::fs::write(&db_path, "dummy")?;
            self.db_path = Some(db_path);
        }

        Ok(())
    }
}

/// Context provided to filesystem test functions
#[derive(Debug, Clone)]
pub struct FilesystemTestContext<'a> {
    pub source_dir: Option<&'a Path>,
    pub mount_dir: Option<&'a Path>,
    pub db_path: Option<&'a Path>,
}

/// Builder for FilesystemTestHarness
pub struct FilesystemTestHarnessBuilder {
    source_dir: Option<PathBuf>,
    mount_dir: Option<PathBuf>,
    db_path: Option<PathBuf>,
}

impl FilesystemTestHarnessBuilder {
    pub fn new() -> Self {
        Self {
            source_dir: None,
            mount_dir: None,
            db_path: None,
        }
    }

    pub fn source_dir<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.source_dir = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn mount_dir<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.mount_dir = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn db_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.db_path = Some(path.as_ref().to_path_buf());
        self
    }
}

impl TestHarnessBuilder for FilesystemTestHarnessBuilder {
    type Harness = FilesystemTestHarness;

    fn build(self) -> io::Result<Self::Harness> {
        Ok(FilesystemTestHarness {
            resources: TestResources::new(),
            source_dir: self.source_dir,
            mount_dir: self.mount_dir,
            db_path: self.db_path,
        })
    }
}

impl Default for FilesystemTestHarnessBuilder {
    fn default() -> Self {
        Self::new()
    }
}
