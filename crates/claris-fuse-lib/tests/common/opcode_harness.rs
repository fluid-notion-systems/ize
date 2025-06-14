//! OpCode queue test harness for Claris-FUSE
//!
//! Provides minimal infrastructure for testing the OpCode queue system.

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

/// OpCode queue test harness
pub struct OpCodeQueueHarness {
    resources: TestResources,
    storage: MockStorage,
    queue_size: usize,
}

impl OpCodeQueueHarness {
    /// Create a new OpCode queue test harness
    pub fn new() -> Self {
        Self {
            resources: TestResources::new(),
            storage: MockStorage::new(),
            queue_size: 10,
        }
    }
}

impl TestHarness for OpCodeQueueHarness {
    type Context<'a> = OpCodeQueueContext<'a>;

    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where
        F: FnOnce(Self::Context<'_>) -> R,
    {
        let ctx = OpCodeQueueContext {
            storage: &self.storage,
            queue_size: self.queue_size,
        };
        test_fn(ctx)
    }
}

/// Context provided to OpCode queue test functions
#[derive(Debug)]
pub struct OpCodeQueueContext<'a> {
    pub storage: &'a MockStorage,
    pub queue_size: usize,
}

/// Builder for OpCodeQueueHarness
pub struct OpCodeQueueHarnessBuilder {
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

impl TestHarnessBuilder for OpCodeQueueHarnessBuilder {
    type Harness = OpCodeQueueHarness;

    fn build(self) -> io::Result<Self::Harness> {
        let mut storage = MockStorage::new();
        if let Some(pattern) = self.storage_fail_pattern {
            storage = storage.with_failure_pattern(&pattern);
        }

        Ok(OpCodeQueueHarness {
            resources: TestResources::new(),
            storage,
            queue_size: self.queue_size,
        })
    }
}

impl Default for OpCodeQueueHarnessBuilder {
    fn default() -> Self {
        Self::new()
    }
}
