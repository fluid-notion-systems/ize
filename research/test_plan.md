# Ize Test Plan

## Overview

This document outlines the comprehensive testing strategy for Ize, organized by test type and component. All tests will use the harness framework to eliminate duplication and ensure consistency.

## Test Categories

### 1. Unit Tests (`tests/unit/`)

#### Path Management Tests
**File**: `path_manager_test.rs`
- Path transformation (absolute ↔ relative)
- Inode allocation and retrieval
- Path normalization edge cases
- Special character handling
- Root directory handling
- Path cache consistency

#### Op Queue Tests
**File**: `op_queue_test.rs`
- Basic enqueue/dequeue operations
- Queue capacity limits
- Thread-safe concurrent access
- Batch processing
- Priority handling (if implemented)
- Queue overflow behavior

#### Storage Interface Tests
**File**: `storage_trait_test.rs`
- Mock storage implementation
- Operation serialization/deserialization
- Error handling and recovery
- Transaction boundaries
- Storage initialization

#### CLI Parsing Tests
**File**: `cli_test.rs`
- Command parsing validation
- Argument validation
- Error message clarity
- Help text generation
- Subcommand routing

### 2. Integration Tests (`tests/integration/`)

#### PassthroughFS Tests
**File**: `passthrough_integration_test.rs`
- File operations with mock storage
- Directory operations
- Attribute preservation
- Permission handling
- Symlink support
- Database file hiding

#### Storage Backend Tests
**File**: `storage_backends_test.rs`
- SQLite implementation
- Concurrent write handling
- Transaction isolation
- Query performance
- Schema migrations
- Data integrity after crashes

#### Op Processing Pipeline Tests
**File**: `op_pipeline_test.rs`
- Queue → Storage flow
- Batch processing efficiency
- Error recovery mechanisms
- Backpressure handling
- Operation ordering guarantees

### 3. Functional Tests (`tests/functional/`)

#### Mount Operations Tests
**File**: `mount_operations_test.rs`
- Basic mount/unmount
- Read-only mount
- Mount with existing data
- Unmount cleanup
- Signal handling (SIGINT, SIGTERM)
- Multiple mount points

#### CLI Workflow Tests
**File**: `cli_workflows_test.rs`
- Init → Mount → Operations → Unmount
- History viewing (when implemented)
- File restoration (when implemented)
- Error scenarios
- User feedback messages

### 4. Property-Based Tests (`tests/property/`)

#### Filesystem Invariants Tests
**File**: `filesystem_invariants_test.rs`
```rust
// Properties to test:
// - Path round-trip: relative → absolute → relative
// - Inode uniqueness across all paths
// - File content preservation through operations
// - Metadata consistency
// - Directory structure integrity
```

#### Op Properties Tests
**File**: `op_properties_test.rs`
```rust
// Properties to test:
// - Operations are idempotent where applicable
// - Operation order preserved in queue
// - No data loss under concurrent access
// - Serialization round-trip preservation
```

### 5. Performance Benchmarks (`tests/benchmarks/`)

#### Operation Throughput Benchmarks
**File**: `operation_throughput_bench.rs`
- Single-threaded enqueue rate
- Multi-threaded enqueue rate
- Dequeue batch processing speed
- Queue memory overhead
- Lock contention measurement

#### Filesystem Performance Benchmarks
**File**: `filesystem_bench.rs`
- File read/write throughput
- Directory listing performance
- Metadata operation latency
- Large file handling
- Many small files scenario

#### Storage Backend Benchmarks
**File**: `storage_bench.rs`
- Write transaction throughput
- Query performance by path
- Index efficiency
- Storage space overhead
- Concurrent access scaling

## Test Data Requirements

### Fixtures (`tests/fixtures/`)
- Sample directory structures
- Various file types (text, binary, empty)
- Permission test cases
- Symlink test scenarios
- Large file samples (generated)

### Test Utilities (`tests/common/`)
- File content generators
- Directory structure builders
- Performance measurement helpers
- Assertion helpers
- Mock implementations

## Test Execution Strategy

### Phase 1: Core Unit Tests
1. Implement path management tests
2. Create Op queue tests
3. Add storage interface tests
4. Validate CLI parsing

### Phase 2: Integration Layer
1. PassthroughFS with mocks
2. Storage backend validation
3. Pipeline flow testing

### Phase 3: End-to-End Testing
1. Mount operation scenarios
2. CLI workflow validation
3. Error scenario coverage

### Phase 4: Quality Assurance
1. Property-based test implementation
2. Performance baseline establishment
3. Stress testing scenarios

## Coverage Goals

- **Unit Tests**: 90%+ line coverage
- **Integration Tests**: All component boundaries
- **Functional Tests**: Critical user paths
- **Property Tests**: Key invariants
- **Benchmarks**: Performance-critical paths

## Test Environment Requirements

### Local Development
- Temporary directories for isolation
- Mock FUSE operations where possible
- In-memory SQLite for speed

### CI Environment
- Real FUSE mounting (privileged)
- Parallel test execution
- Performance regression detection
- Coverage reporting

## Special Considerations

### FUSE-Specific Testing
- Some tests require actual FUSE mounting
- Need privileged access in CI
- Alternative: abstract filesystem operations trait

### Concurrent Testing
- Tests must be isolated (unique paths)
- Resource cleanup critical
- Lock files for shared resources

### Platform Differences
- Linux: Full FUSE support
- macOS: OSXFUSE considerations
- Windows: Future WSL2 support

## Success Criteria

1. **Reliability**: No flaky tests
2. **Speed**: Full suite < 30 seconds
3. **Coverage**: > 85% overall
4. **Maintainability**: Clear test names and purposes
5. **Debuggability**: Helpful failure messages
