//! Base test harness framework for Claris-FUSE tests
//!
//! This module provides the foundational traits and structures for creating
//! clean, DRY test harnesses that eliminate duplicate setup code.

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Core trait for all test harnesses
pub trait TestHarness {
    /// The context type that will be passed to test functions
    type Context<'a>
    where
        Self: 'a;

    /// Execute a test function with the harness context
    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(Self::Context<'_>) -> R;

    /// Setup method called before test execution
    fn setup(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Teardown method called after test execution
    fn teardown(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Base resources that most test harnesses will need
#[derive(Debug)]
pub struct TestResources {
    /// Temporary directories that will be cleaned up automatically
    pub temp_dirs: Vec<TempDir>,
    /// Additional paths that need tracking
    pub paths: Vec<PathBuf>,
}

impl TestResources {
    pub fn new() -> Self {
        Self {
            temp_dirs: Vec::new(),
            paths: Vec::new(),
        }
    }

    /// Create a new temporary directory
    pub fn create_temp_dir(&mut self) -> io::Result<&Path> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path();
        self.temp_dirs.push(temp_dir);
        Ok(path)
    }

    /// Create a temporary directory with a specific prefix
    pub fn create_temp_dir_with_prefix(&mut self, prefix: &str) -> io::Result<&Path> {
        let temp_dir = tempfile::Builder::new().prefix(prefix).tempdir()?;
        let path = temp_dir.path();
        self.temp_dirs.push(temp_dir);
        Ok(path)
    }

    /// Add a path to be tracked (but not automatically cleaned up)
    pub fn track_path<P: AsRef<Path>>(&mut self, path: P) {
        self.paths.push(path.as_ref().to_path_buf());
    }
}

impl Default for TestResources {
    fn default() -> Self {
        Self::new()
    }
}

/// Result type for test harness operations
pub type TestResult<T> = Result<T, TestError>;

/// Error type for test harness operations
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Setup failed: {0}")]
    SetupFailed(String),

    #[error("Test assertion failed: {0}")]
    AssertionFailed(String),

    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    #[error("Operation timeout: {0}")]
    Timeout(String),

    #[error("Unexpected error: {0}")]
    Other(String),
}

/// Builder pattern for constructing test harnesses
pub trait TestHarnessBuilder: Sized {
    type Harness: TestHarness;

    /// Build the test harness
    fn build(self) -> io::Result<Self::Harness>;
}

/// Utility functions for test assertions
pub mod assertions {
    use super::*;
    use std::time::{Duration, Instant};

    /// Assert that a condition becomes true within a timeout
    pub fn assert_eventually<F>(
        condition: F,
        timeout: Duration,
        check_interval: Duration,
        message: &str,
    ) -> TestResult<()>
    where
        F: Fn() -> bool,
    {
        let start = Instant::now();

        while start.elapsed() < timeout {
            if condition() {
                return Ok(());
            }
            std::thread::sleep(check_interval);
        }

        Err(TestError::Timeout(format!(
            "Condition not met within {:?}: {}",
            timeout, message
        )))
    }

    /// Assert that two paths point to the same content
    pub fn assert_same_content(path1: &Path, path2: &Path) -> TestResult<()> {
        use std::fs;

        let content1 = fs::read(path1).map_err(|e| {
            TestError::Io(io::Error::new(
                e.kind(),
                format!("Reading {}: {}", path1.display(), e),
            ))
        })?;
        let content2 = fs::read(path2).map_err(|e| {
            TestError::Io(io::Error::new(
                e.kind(),
                format!("Reading {}: {}", path2.display(), e),
            ))
        })?;

        if content1 != content2 {
            return Err(TestError::AssertionFailed(format!(
                "File contents differ: {} vs {}",
                path1.display(),
                path2.display()
            )));
        }

        Ok(())
    }

    /// Assert that a directory contains expected files
    pub fn assert_dir_contains(dir: &Path, expected_files: &[&str]) -> TestResult<()> {
        use std::fs;

        let entries = fs::read_dir(dir).map_err(|e| {
            TestError::Io(io::Error::new(
                e.kind(),
                format!("Reading directory {}: {}", dir.display(), e),
            ))
        })?;

        let mut found_files: Vec<String> = entries
            .filter_map(|entry| {
                entry
                    .ok()
                    .and_then(|e| e.file_name().to_str().map(|s| s.to_string()))
            })
            .collect();

        found_files.sort();
        let mut expected = expected_files.to_vec();
        expected.sort();

        if found_files != expected {
            return Err(TestError::AssertionFailed(format!(
                "Directory {} contains {:?}, expected {:?}",
                dir.display(),
                found_files,
                expected
            )));
        }

        Ok(())
    }
}

/// Macro to simplify test harness usage
#[macro_export]
macro_rules! test_with_harness {
    ($harness_type:ty, $builder:expr, |$ctx:ident| $body:expr) => {{
        let mut harness: $harness_type = $builder.build()?;
        harness.setup()?;
        let result = harness.test_with(|$ctx| $body);
        harness.teardown()?;
        result
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    // Example harness implementation for testing
    struct SimpleHarness {
        resources: TestResources,
        value: i32,
    }

    impl TestHarness for SimpleHarness {
        type Context<'a> = (&'a TestResources, i32);

        fn test_with<F, R>(&mut self, test_fn: F) -> R
        where
            F: FnOnce(Self::Context<'_>) -> R,
        {
            test_fn((&self.resources, self.value))
        }
    }

    #[test]
    fn test_basic_harness() {
        let mut harness = SimpleHarness {
            resources: TestResources::new(),
            value: 42,
        };

        harness.test_with(|(resources, value)| {
            assert_eq!(*value, 42);
            assert!(resources.temp_dirs.is_empty());
        });
    }

    #[test]
    fn test_assert_eventually() {
        use assertions::assert_eventually;
        use std::sync::{Arc, Mutex};

        let counter = Arc::new(Mutex::new(0));
        let counter_clone = Arc::clone(&counter);

        // Spawn a thread that increments the counter after 50ms
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            *counter_clone.lock().unwrap() = 1;
        });

        // Assert that counter becomes 1 within 100ms
        assert_eventually(
            || *counter.lock().unwrap() == 1,
            Duration::from_millis(100),
            Duration::from_millis(10),
            "Counter should become 1",
        )
        .unwrap();
    }
}
