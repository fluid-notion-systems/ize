//! Example test demonstrating harness usage patterns
//!
//! This file shows how to use the test harness framework for writing
//! clean, maintainable tests without duplicate setup code.

use crate::common::{
    FilesystemTestHarnessBuilder, OpQueueHarnessBuilder, TestHarness, TestHarnessBuilder,
};

#[test]
fn test_filesystem_harness_basic_usage() -> std::io::Result<()> {
    // Create harness using builder pattern
    let mut harness = FilesystemTestHarnessBuilder::new().build()?;

    // Setup is called automatically
    harness.setup()?;

    // Run test with harness context
    harness.test_with(|ctx| {
        // Context provides access to test resources
        assert!(ctx.source_dir.is_some());
        assert!(ctx.mount_dir.is_some());
        assert!(ctx.db_path.is_some());

        // Paths are automatically cleaned up
        let source = ctx.source_dir.unwrap();
        assert!(source.exists());
    });

    // Teardown happens automatically
    harness.teardown()?;
    Ok(())
}

#[test]
fn test_op_queue_harness_usage() -> std::io::Result<()> {
    // Create harness with custom configuration
    let mut harness = OpQueueHarnessBuilder::new().queue_size(100).build()?;

    harness.test_with(|ctx| {
        // Access queue configuration
        assert_eq!(ctx.queue_size, 100);

        // Mock storage is available
        assert_eq!(ctx.storage.operation_count(), 0);
    });

    Ok(())
}

#[test]
fn test_harness_with_multiple_operations() -> std::io::Result<()> {
    let mut harness = FilesystemTestHarnessBuilder::new().build()?;
    harness.setup()?;

    // Can run multiple test blocks with same harness
    harness.test_with(|ctx| {
        let source = ctx.source_dir.unwrap();
        std::fs::write(source.join("test1.txt"), "content1").unwrap();
    });

    harness.test_with(|ctx| {
        let source = ctx.source_dir.unwrap();
        // Previous test's file is still there
        assert!(source.join("test1.txt").exists());

        // Add another file
        std::fs::write(source.join("test2.txt"), "content2").unwrap();
    });

    harness.teardown()?;
    Ok(())
}

#[test]
fn test_harness_with_failure_simulation() -> std::io::Result<()> {
    // Configure harness to simulate failures
    let mut harness = OpQueueHarnessBuilder::new()
        .with_storage_failure("error")
        .build()?;

    harness.test_with(|ctx| {
        // Storage is configured to fail on certain patterns
        let storage = ctx.storage;
        // In real implementation, this would trigger the failure pattern
        assert!(storage.operation_count() == 0);
    });

    Ok(())
}

// Example using the test_with_harness! macro (when implemented)
#[test]
#[ignore = "Macro not yet implemented"]
fn test_with_macro_example() {
    // This shows the intended usage pattern for the macro
    // test_with_harness!(FilesystemTestHarness,
    //     FilesystemTestHarnessBuilder::new(),
    //     |ctx| {
    //         assert!(ctx.source_dir.is_some());
    //     }
    // );
}
