//! Common test utilities and harnesses for Ize
//!
//! This module provides reusable test infrastructure to eliminate
//! duplicate setup code and create focused, maintainable tests.

pub mod filesystem_harness;
pub mod harness;
pub mod op_harness;

// Re-export commonly used items
pub use harness::{
    assertions, TestError, TestHarness, TestHarnessBuilder, TestResources, TestResult,
};

pub use filesystem_harness::{
    FilesystemTestContext, FilesystemTestHarness, FilesystemTestHarnessBuilder,
};

pub use op_harness::{MockStorage, OpQueueContext, OpQueueHarness, OpQueueHarnessBuilder};

// Re-export the test macro
pub use crate::test_with_harness;
