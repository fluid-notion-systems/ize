# Ize Integration Tests

This directory contains integration tests that mount the actual filesystem using FUSE and verify that operations are correctly tracked and persisted.

## Test Approach

Unlike unit tests that mock components, these integration tests:

1. **Mount Real Filesystems**: Use `fuser::spawn_mount2` to programmatically mount the filesystem
2. **Perform Real Operations**: Execute actual file operations through the mounted filesystem
3. **Verify Tracking**: Check that operations are correctly recorded in the backend storage
4. **Test End-to-End**: Validate the complete flow from FUSE operation to storage persistence

## Test Files

### `write_operations_test.rs`
Tests for file write operations including:
- Simple file creation and writing
- Multiple file writes tracking
- File append operations
- Large file handling
- Directory creation and population
- Metadata operations (permissions, truncation)

### `operation_tracking_test.rs`
Comprehensive operation tracking tests:
- File CRUD operations (Create, Read, Update, Delete)
- Directory operations (mkdir, rmdir, nested creation)
- Metadata changes (chmod, timestamps, truncate)
- Rename operations
- Complex operation sequences
- Concurrent operation handling

## Test Harness Architecture

The tests use a harness-based approach to eliminate boilerplate:

```rust
struct FilesystemMountHarness {
    source_dir: TempDir,      // Source directory being versioned
    mount_dir: TempDir,       // Mount point for FUSE filesystem
    db_path: PathBuf,         // Path to Ize database
    _session: BackgroundSession, // FUSE session handle
}
```

Key features:
- **Automatic Cleanup**: Temp directories and mounts are cleaned up on drop
- **Programmatic Mounting**: No need for external `fusermount` commands
- **Context Objects**: Specialized contexts for different operation types
- **Verification Helpers**: Methods to check operations in both mount and source

## How Tests Work

1. **Setup Phase**:
   - Create temporary directories for source and mount
   - Initialize the Ize database file
   - Create PassthroughFS instance
   - Mount filesystem using `fuser::spawn_mount2`

2. **Operation Phase**:
   - Perform file operations through the mount point
   - Operations are intercepted by PassthroughFS
   - PassthroughFS records operations to storage

3. **Verification Phase**:
   - Check files exist in source directory
   - Verify operation was tracked in storage
   - Validate file contents and metadata

## Running the Tests

```bash
# Run all integration tests
cargo test --test "*" -- --test-threads=1

# Run specific test file
cargo test --test write_operations_test

# Run with output for debugging
cargo test --test operation_tracking_test -- --nocapture
```

**Important**: Use `--test-threads=1` to avoid mount conflicts when running multiple tests.

## Requirements

- **Linux or macOS**: FUSE support required
- **Permissions**: May need user_allow_other in /etc/fuse.conf
- **FUSE Installation**:
  - Linux: `sudo apt-get install fuse` or equivalent
  - macOS: Install OSXFUSE/macFUSE

## Test Patterns

### Basic Operation Test
```rust
#[test]
fn test_file_create_operation_tracked() {
    let harness = OperationTrackingHarness::new().unwrap().mount().unwrap();

    // Perform operation through mount
    fs::write(harness.mount_path().join("file.txt"), b"content").unwrap();

    // Wait for FUSE to process
    thread::sleep(Duration::from_millis(100));

    // Verify in source
    assert!(harness.source_path().join("file.txt").exists());

    // Verify tracked
    assert!(harness.verify_operation_tracked(Path::new("file.txt"), "create"));
}
```

### Complex Sequence Test
```rust
#[test]
fn test_complex_operations() {
    let harness = Harness::new().mount();

    harness.test_write_operations(|ctx| {
        ctx.write_file("doc.txt", b"content").unwrap();
        ctx.append_file("doc.txt", b" more").unwrap();
        ctx.verify_in_source("doc.txt", b"content more").unwrap();
    });
}
```

## Debugging Tips

1. **Mount Issues**: Check if mount point is accessible after mounting
2. **Timing**: Some tests use `thread::sleep` to ensure operations complete
3. **Permissions**: Ensure test has permission to mount FUSE filesystems
4. **Cleanup**: Failed tests may leave mounts; use `fusermount -u <path>` to clean up

## Future Enhancements

- [ ] Storage backend verification (once implemented)
- [ ] Performance benchmarks during operations
- [ ] Stress testing with many concurrent operations
- [ ] Cross-platform testing (Windows via WSL2)
- [ ] Network filesystem testing
