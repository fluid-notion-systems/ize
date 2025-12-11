# Ize Test Suite

This directory contains the comprehensive test suite for Ize, built on a clean, DRY testing framework that eliminates duplicate setup code.

## ⚠️ Important: FUSE Integration Tests are Ignored by Default

Integration tests that require FUSE mounting are **disabled by default** because they:
- Require FUSE to be installed and configured
- Need elevated permissions or specific system configuration
- Can be slow and resource-intensive
- May interfere with other system processes

## Running Tests

### Run Unit Tests (Default - Fast)

```bash
# Run all non-ignored tests (recommended for development)
cargo test --package ize-lib

# Run only library unit tests
cargo test --package ize-lib --lib
```

### Run Integration Tests (Requires FUSE)

```bash
# Run ALL tests including ignored integration tests
cargo test --package ize-lib -- --ignored --test-threads=1

# Run with verbose output
cargo test --package ize-lib -- --ignored --test-threads=1 --nocapture

# Run specific ignored test
cargo test --package ize-lib test_name -- --ignored --test-threads=1
```

### Why Single-Threading for Integration Tests?

The FUSE integration tests mount filesystems, which:
1. Require exclusive access to mount points
2. Can interfere with each other if run in parallel
3. Need proper cleanup between test runs

### Test Categories

```bash
# Unit tests only (fast, can run in parallel)
cargo test --package ize-lib --lib

# All tests including ignored integration tests
cargo test --package ize-lib -- --ignored --test-threads=1

# Benchmarks
cargo bench --package ize-lib
```

## Test Organization

```
tests/
├── common/           # Shared test harnesses and utilities
│   ├── harness.rs    # Base TestHarness trait and utilities
│   ├── filesystem_harness.rs  # Filesystem-specific harness
│   ├── op_harness.rs # Op queue testing harness
│   └── mod.rs        # Module exports
├── unit/            # Fast, isolated component tests
├── functional/      # Feature-level tests with real operations
├── integration/     # End-to-end system tests
├── benchmarks/      # Performance benchmarks
└── README.md        # This file
```

## Core Philosophy

Our testing approach follows these principles:

1. **No Duplication**: Setup code is written once per harness type
2. **Focused Tests**: Test functions contain only the behavior being tested
3. **Type Safety**: Context structs provide safe APIs
4. **Automatic Cleanup**: Resources are cleaned up automatically
5. **Fast & Reliable**: No timing dependencies or flaky tests

## Using Test Harnesses

### Basic Pattern

```rust
use crate::common::*;

#[test]
fn test_example() -> io::Result<()> {
    let mut harness = FilesystemTestHarness::new();

    harness.test_with(|ctx| {
        // Your test logic here
        // ctx provides access to test directories and paths
    });

    Ok(())
}
```

### Filesystem Tests

```rust
#[test]
fn test_file_operations() -> io::Result<()> {
    test_with_harness!(
        FilesystemTestHarness,
        FilesystemTestHarnessBuilder::new(),
        |ctx| {
            // ctx.source_dir - source directory path
            // ctx.mount_dir - mount point path
            // ctx.db_path - database file path

            let test_file = ctx.source_dir.unwrap().join("test.txt");
            std::fs::write(&test_file, "Hello, world!")?;

            let content = std::fs::read_to_string(&test_file)?;
            assert_eq!(content, "Hello, world!");

            Ok::<(), io::Error>(())
        }
    )?;
    Ok(())
}
```

### Op Queue Tests

```rust
#[test]
fn test_op_queue_processing() -> io::Result<()> {
    test_with_harness!(
        OpQueueHarness,
        OpQueueHarnessBuilder::new()
            .queue_size(100)
            .with_storage_failure("error"),
        |ctx| {
            // ctx.storage - mock storage backend
            // ctx.queue_size - configured queue size

            // Test queue operations
            assert_eq!(ctx.storage.operation_count(), 0);

            // Add operations and verify processing
        }
    )?;
    Ok(())
}
```

### Using Test Utilities

```rust
use crate::common::assertions::*;

#[test]
fn test_async_operation() -> TestResult<()> {
    // Wait for condition with timeout
    assert_eventually(
        || check_some_condition(),
        Duration::from_secs(5),
        Duration::from_millis(100),
        "Operation should complete"
    )?;

    // Compare file contents
    assert_same_content(
        Path::new("file1.txt"),
        Path::new("file2.txt")
    )?;

    // Verify directory contents
    assert_dir_contains(
        Path::new("test_dir"),
        &["file1.txt", "file2.txt", "subdir"]
    )?;

    Ok(())
}
```

## Writing New Tests

### 1. Choose the Right Category

- **Unit Tests**: Test individual functions or small components in isolation
- **Functional Tests**: Test complete features (e.g., file versioning, mounting)
- **Integration Tests**: Test the full system end-to-end
- **Benchmarks**: Measure performance of critical operations

### 2. Use the Appropriate Harness

- `FilesystemTestHarness`: For filesystem operations
- `OpQueueHarness`: For operation queue testing
- `TestHarness` trait: Create custom harnesses for new components

### 3. Keep Tests Focused

```rust
// Good: Single responsibility
#[test]
fn test_create_file_adds_to_history() { /* ... */ }

// Bad: Testing too many things
#[test]
fn test_everything() { /* ... */ }
```

### 4. Use Descriptive Names

```rust
// Good: Clear what's being tested
#[test]
fn test_rename_updates_path_in_operations_table() { /* ... */ }

// Bad: Unclear
#[test]
fn test_rename() { /* ... */ }
```

## Adding New Test Harnesses

To create a new test harness:

1. Implement the `TestHarness` trait
2. Define a context struct for test functions
3. Create a builder if configuration is needed
4. Add to `common/mod.rs` exports

Example:

```rust
pub struct MyComponentHarness {
    resources: TestResources,
    component: MyComponent,
}

impl TestHarness for MyComponentHarness {
    type Context<'a> = MyComponentContext<'a>;

    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where F: FnOnce(Self::Context<'_>) -> R
    {
        let ctx = MyComponentContext {
            component: &mut self.component,
        };
        test_fn(ctx)
    }
}
```

## Best Practices

1. **Always use harnesses** - Don't write setup/teardown in test functions
2. **Test one thing** - Each test should verify a single behavior
3. **Use meaningful assertions** - Assert on the actual vs expected outcome
4. **Handle errors properly** - Use `?` operator and return `Result` types
5. **Mock external dependencies** - Use mock implementations for storage, network, etc.
6. **Avoid sleep/timing** - Use `assert_eventually` for async operations
7. **Clean up resources** - Let harnesses handle cleanup automatically

## Property-Based Testing

For complex invariants, use property-based testing:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_path_normalization(path in "\\PC*") {
        let normalized = normalize_path(&path);
        // Properties that should always hold
        prop_assert!(!normalized.contains("//"));
        prop_assert!(!normalized.ends_with("/"));
    }
}
```

## Debugging Tests

1. Use `--nocapture` to see println! output
2. Set `RUST_LOG=debug` for detailed logging
3. Use `RUST_BACKTRACE=1` for stack traces
4. Add `.unwrap()` temporarily to get better error locations
5. Use the harness context to inspect state

## Contributing

When adding new tests:

1. Follow the existing patterns
2. Add documentation for complex test scenarios
3. Ensure tests are deterministic
4. Keep tests fast (< 1 second for unit tests)
5. Update this README if adding new harnesses or patterns
