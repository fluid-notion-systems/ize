# Testing and Debugging Strategy for Claris-FUSE

## Overview

This document outlines comprehensive approaches for testing and debugging the Claris-FUSE filesystem implementation. The goal is to ensure reliability, correctness, and performance of the FUSE filesystem operations.

## Automated Testing Framework

### Unit Tests

1. **Path Translation Testing**
   - Test `real_path` translation with various path inputs
   - Test edge cases like empty paths, absolute paths, and relative paths
   - Verify correct handling of special characters

2. **Inode Mapping Tests**
   - Test `get_inode_for_path` and `get_path_for_inode` functions
   - Verify consistent inode allocation
   - Check edge cases (root directory, non-existent paths)

3. **Attribute Conversion Tests**
   - Test `stat_to_fuse_attr` with various file types
   - Verify correct conversion of timestamps, permissions, and file types

### Integration Tests

1. **Filesystem Initialization**
   - Test database creation and schema setup
   - Verify error handling for invalid paths
   - Test read-only mode initialization

2. **Mount/Unmount Tests**
   - Test mounting with various options
   - Verify clean unmounting
   - Test behavior with multiple mount points

3. **Core Operations Tests**
   - Test each file and directory operation individually
   - Verify proper error handling
   - Test operations in read-only mode

### Round-trip Testing

1. **Data Integrity Tests**
   - Write data patterns to files
   - Unmount and remount the filesystem
   - Read back and verify data integrity

2. **Metadata Preservation**
   - Create files with specific attributes
   - Verify attributes are preserved across mount/unmount cycles

3. **Concurrency Testing**
   - Test multiple simultaneous operations
   - Verify thread safety of shared data structures

## Specific Test Cases

### File Operations

```rust
#[test]
fn test_file_create_read_write() {
    // Setup test environment
    let temp_dir = tempdir().unwrap();
    let mount_dir = tempdir().unwrap();
    
    // Mount filesystem
    let fs = mount_test_filesystem(temp_dir.path(), mount_dir.path());
    
    // Test file creation
    let test_file = mount_dir.path().join("test.txt");
    let test_data = "Hello, world!";
    fs::write(&test_file, test_data).expect("Failed to write to file");
    
    // Test file reading
    let read_data = fs::read_to_string(&test_file).expect("Failed to read file");
    assert_eq!(read_data, test_data);
    
    // Test file attributes
    let metadata = fs::metadata(&test_file).expect("Failed to get metadata");
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), test_data.len() as u64);
    
    // Unmount and verify changes persisted
    unmount_test_filesystem(fs);
    
    // Verify file exists in original location
    let source_file = temp_dir.path().join("test.txt");
    assert!(source_file.exists());
    
    // Verify content is correct
    let source_data = fs::read_to_string(source_file).expect("Failed to read source file");
    assert_eq!(source_data, test_data);
}
```

### Directory Operations

```rust
#[test]
fn test_directory_operations() {
    // Setup and mount
    let temp_dir = tempdir().unwrap();
    let mount_dir = tempdir().unwrap();
    let fs = mount_test_filesystem(temp_dir.path(), mount_dir.path());
    
    // Test directory creation
    let test_dir = mount_dir.path().join("test_dir");
    fs::create_dir(&test_dir).expect("Failed to create directory");
    
    // Test nested directory creation
    let nested_dir = test_dir.join("nested");
    fs::create_dir(&nested_dir).expect("Failed to create nested directory");
    
    // Add a file to the nested directory
    let nested_file = nested_dir.join("test.txt");
    fs::write(&nested_file, "Nested file").expect("Failed to write to nested file");
    
    // Test directory reading
    let entries = fs::read_dir(&test_dir)
        .expect("Failed to read directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to collect directory entries");
    
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), "nested");
    
    // Test directory removal (recursive)
    fs::remove_dir_all(&test_dir).expect("Failed to remove directory");
    assert!(!test_dir.exists());
    
    // Unmount
    unmount_test_filesystem(fs);
    
    // Verify changes persisted
    let source_dir = temp_dir.path().join("test_dir");
    assert!(!source_dir.exists());
}
```

### Metadata Operations

```rust
#[test]
fn test_metadata_operations() {
    // Setup and mount
    let temp_dir = tempdir().unwrap();
    let mount_dir = tempdir().unwrap();
    let fs = mount_test_filesystem(temp_dir.path(), mount_dir.path());
    
    // Create a test file
    let test_file = mount_dir.path().join("permissions.txt");
    fs::write(&test_file, "Test").expect("Failed to write to file");
    
    // Test permissions change
    let mode = 0o644; // rw-r--r--
    fs::set_permissions(&test_file, fs::Permissions::from_mode(mode))
        .expect("Failed to set permissions");
    
    // Verify permissions
    let metadata = fs::metadata(&test_file).expect("Failed to get metadata");
    assert_eq!(metadata.permissions().mode() & 0o777, mode);
    
    // Unmount
    unmount_test_filesystem(fs);
    
    // Verify permissions persisted
    let source_file = temp_dir.path().join("permissions.txt");
    let source_metadata = fs::metadata(&source_file).expect("Failed to get source metadata");
    assert_eq!(source_metadata.permissions().mode() & 0o777, mode);
}
```

## Structured Logging and Debugging

### Log Configuration

Add a configurable logging system that captures different verbosity levels:

```rust
fn setup_logging(log_path: Option<&Path>, verbosity: u8) -> Result<()> {
    let log_level = match verbosity {
        0 => LevelFilter::Error,
        1 => LevelFilter::Warn,
        2 => LevelFilter::Info,
        3 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    
    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![
        TermLogger::new(
            log_level,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        )
    ];
    
    if let Some(path) = log_path {
        loggers.push(
            WriteLogger::new(
                log_level,
                Config::default(),
                File::create(path)?
            )
        );
    }
    
    CombinedLogger::init(loggers)?;
    
    Ok(())
}
```

### Debug Flags

Add environment variables to control debugging:

- `CLARIS_FUSE_LOG_LEVEL`: Controls log verbosity (0-4)
- `CLARIS_FUSE_LOG_FILE`: Path to write logs to
- `CLARIS_FUSE_DEBUG_MODE`: Enable debug mode with extra checks

### Debug Virtual Directory

Implement a special `/debug` directory in the mounted filesystem that exposes internal state:

- `/debug/inodes.txt`: Shows inode-to-path mappings
- `/debug/stats.txt`: Shows operation statistics
- `/debug/config.txt`: Shows current configuration

## Debug CLI Tool

Create a dedicated CLI tool for debugging:

```rust
#[derive(Parser)]
struct DebugCli {
    #[clap(subcommand)]
    command: DebugCommand,
}

#[derive(Subcommand)]
enum DebugCommand {
    /// Show filesystem status
    Status {
        #[arg(value_name = "MOUNT_POINT")]
        mount_point: PathBuf,
    },
    
    /// Run test operations
    Test {
        #[arg(value_name = "MOUNT_POINT")]
        mount_point: PathBuf,
        
        #[arg(value_name = "OPERATION")]
        operation: String,
    },
    
    /// Analyze logs
    Logs {
        #[arg(value_name = "LOG_FILE")]
        log_file: PathBuf,
        
        #[arg(long)]
        show_errors_only: bool,
    },
}
```

## Benchmark Tests

Implement performance tests to measure:

1. **Throughput Tests**
   - Measure read/write speeds with various file sizes
   - Compare against native filesystem performance

2. **Latency Tests**
   - Measure operation response time
   - Identify potential bottlenecks

3. **Scalability Tests**
   - Test with many small files vs. few large files
   - Measure memory usage under different scenarios

## Manual Testing Checklist

- [ ] Mount filesystem with various options
- [ ] Create, read, update, and delete files
- [ ] Create and navigate directory structures
- [ ] Test symbolic and hard links
- [ ] Test with different file permissions
- [ ] Test concurrent operations from multiple processes
- [ ] Test edge cases (very long paths, special characters)
- [ ] Test error cases (read-only filesystem, permission denied)
- [ ] Test system resilience (unexpected unmount, process termination)

## Continuous Integration

Set up CI workflows to:

1. Run all tests on each commit
2. Run benchmarks and compare with baseline
3. Check for performance regressions
4. Generate test coverage reports

## Regression Testing

Maintain a test suite that includes previous bug fixes:

1. Document each bug with a reproducible test case
2. Ensure the test suite covers all fixed bugs
3. Run regression tests on each significant change

## Testing with Real-World Applications

Test with real-world applications that use the filesystem:

- Text editors (vim, nano, etc.)
- Compilers and build systems
- Version control systems (git)
- Media players and editors