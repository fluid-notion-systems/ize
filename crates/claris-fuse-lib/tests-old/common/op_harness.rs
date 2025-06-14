//! Op queue test harness for Claris-FUSE
//!
//! Provides minimal infrastructure for testing the Op queue system.

use super::harness::{TestHarness, TestHarnessBuilder, TestResources};
use std::io;
use std::sync::{Arc, Mutex};

/// Mock storage implementation for testing
#[derive(Clone, Debug)]
pub struct MockStorage {
    operations: Arc<Mutex<Vec<String>>>,
    fail_pattern: Option<String>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            operations: Arc::new(Mutex::new(Vec::new())),
            fail_pattern: None,
        }
    }

    pub fn with_failure_pattern(mut self, pattern: &str) -> Self {
        self.fail_pattern = Some(pattern.to_string());
        self
    }

    pub fn operation_count(&self) -> usize {
        self.operations.lock().unwrap().len()
    }

    pub fn get_operations(&self) -> Vec<String> {
        self.operations.lock().unwrap().clone()
    }
}

/// Op queue test harness
pub struct OpQueueHarness {
    resources: TestResources,
    storage: MockStorage,
    queue_size: usize,
}

impl OpQueueHarness {
    /// Create a new Op queue test harness
    pub fn new() -> Self {
        Self {
            resources: TestResources::new(),
            storage: MockStorage::new(),
            queue_size: 10,
        }
    }
}

impl TestHarness for OpQueueHarness {
    type Context<'a> = OpQueueContext<'a>;

    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(Self::Context<'_>) -> R,
    {
        let ctx = OpQueueContext {
            storage: &self.storage,
            queue_size: self.queue_size,
        };
        test_fn(ctx)
    }
}

/// Context provided to Op queue test functions
#[derive(Debug)]
pub struct OpQueueContext<'a> {
    pub storage: &'a MockStorage,
    pub queue_size: usize,
}

/// Builder for OpQueueHarness
pub struct OpQueueHarnessBuilder {
    queue_size: usize,
    storage_fail_pattern: Option<String>,
}

impl OpCodeQueueHarnessBuilder {
    pub fn new() -> Self {
        Self {
            queue_size: 10,
            storage_fail_pattern: None,
        }
    }

    pub fn queue_size(mut self, size: usize) -> Self {
        self.queue_size = size;
        self
    }

    pub fn with_storage_failure(mut self, pattern: &str) -> Self {
        self.storage_fail_pattern = Some(pattern.to_string());
        self
    }
}

impl TestHarnessBuilder for OpQueueHarnessBuilder {
    type Harness = OpQueueHarness;

    fn build(self) -> io::Result<Self::Harness> {
        let mut storage = MockStorage::new();
        if let Some(pattern) = self.storage_fail_pattern {
            storage = storage.with_failure_pattern(&pattern);
        }

        Ok(OpQueueHarness {
            resources: TestResources::new(),
            storage,
            queue_size: self.queue_size,
        })
    }
}

impl Default for OpQueueHarnessBuilder {
    fn default() -> Self {
        Self::new()
    }
}
