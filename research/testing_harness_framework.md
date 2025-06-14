# Testing Harness Framework for Claris-FUSE

## Overview

This document outlines a clean, DRY testing framework based on **test harness structs** that accept **test functions**. This approach eliminates duplicate setup code and creates focused, composable tests.

## Core Philosophy

Instead of repeating setup/teardown in every test, we create **harness structs** that:
1. Handle all setup and resource management
2. Expose clean APIs for test operations  
3. Accept test functions that focus only on the behavior being tested
4. Automatically handle cleanup and error scenarios

## Test Harness Architecture

### Base Harness Pattern

```rust
// Core pattern: Harness struct with test execution method
struct TestHarness {
    // Setup state
    resources: Resources,
}

impl TestHarness {
    fn new() -> Self { /* setup */ }
    
    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where F: FnOnce(&mut TestContext) -> R 
    {
        let mut ctx = TestContext::from(&mut self.resources);
        test_fn(&mut ctx)
    }
}
```

### Filesystem Test Harness

```rust
struct FilesystemTestHarness {
    source_dir: TempDir,
    mount_dir: TempDir,
    db_path: PathBuf,
    filesystem: Option<PassthroughFS>,
    mounted: bool,
}

impl FilesystemTestHarness {
    fn new() -> io::Result<Self> {
        let source_dir = tempdir()?;
        let mount_dir = tempdir()?;
        let db_path = source_dir.path().join("test.db");
        
        // Create minimal DB file
        fs::write(&db_path, "dummy")?;
        
        Ok(Self {
            source_dir,
            mount_dir,
            db_path,
            filesystem: None,
            mounted: false,
        })
    }
    
    fn with_mount(mut self) -> io::Result<Self> {
        let fs = PassthroughFS::new(&self.db_path, self.mount_dir.path())?;
        // Mount in background thread if needed
        self.filesystem = Some(fs);
        self.mounted = true;
        Ok(self)
    }
    
    // Core test execution method
    fn test_with<F, R>(&mut self, test_fn: F) -> R 
    where 
        F: FnOnce(&FilesystemTestContext) -> R 
    {
        let ctx = FilesystemTestContext {
            source_path: self.source_dir.path(),
            mount_path: if self.mounted { Some(self.mount_dir.path()) } else { None },
            db_path: &self.db_path,
        };
        
        test_fn(&ctx)
    }
    
    // Specialized test methods for common scenarios
    fn test_file_ops<F>(&mut self, test_fn: F) 
    where F: FnOnce(&FileOpsContext) 
    {
        self.test_with(|ctx| {
            let file_ops = FileOpsContext::new(ctx);
            test_fn(&file_ops);
        });
    }
    
    fn test_directory_ops<F>(&mut self, test_fn: F)
    where F: FnOnce(&DirectoryOpsContext)
    {
        self.test_with(|ctx| {
            let dir_ops = DirectoryOpsContext::new(ctx);
            test_fn(&dir_ops);
        });
    }
}

// Context structs provide clean APIs for test operations
struct FilesystemTestContext<'a> {
    source_path: &'a Path,
    mount_path: Option<&'a Path>,
    db_path: &'a Path,
}

struct FileOpsContext<'a> {
    ctx: &'a FilesystemTestContext<'a>,
}

impl<'a> FileOpsContext<'a> {
    fn new(ctx: &'a FilesystemTestContext<'a>) -> Self {
        Self { ctx }
    }
    
    fn create_file(&self, name: &str, content: &str) -> io::Result<()> {
        let path = self.get_path(name)?;
        fs::write(path, content)
    }
    
    fn read_file(&self, name: &str) -> io::Result<String> {
        let path = self.get_path(name)?;
        fs::read_to_string(path)
    }
    
    fn verify_in_source(&self, name: &str, expected_content: &str) -> io::Result<()> {
        let source_file = self.ctx.source_path.join(name);
        let actual = fs::read_to_string(source_file)?;
        assert_eq!(actual, expected_content);
        Ok(())
    }
    
    fn get_path(&self, name: &str) -> io::Result<PathBuf> {
        match self.ctx.mount_path {
            Some(mount) => Ok(mount.join(name)),
            None => Ok(self.ctx.source_path.join(name)),
        }
    }
}
```

### OpCode Queue Test Harness

```rust
struct OpCodeQueueHarness {
    queue: OpCodeQueue,
    storage: MockStorage,
    processor: Option<OpCodeProcessor>,
}

impl OpCodeQueueHarness {
    fn new() -> Self {
        let storage = MockStorage::new();
        let queue = OpCodeQueue::new(10);
        
        Self {
            queue,
            storage,
            processor: None,
        }
    }
    
    fn with_processor(mut self) -> Self {
        let processor = OpCodeProcessor::new(
            self.queue.get_shared_queue(),
            Arc::new(Mutex::new(self.storage.clone())),
            100 // 100ms interval
        );
        self.processor = Some(processor);
        self
    }
    
    fn test_with<F, R>(&mut self, test_fn: F) -> R
    where F: FnOnce(&OpCodeQueueContext) -> R
    {
        let ctx = OpCodeQueueContext {
            queue: &self.queue,
            storage: &self.storage,
        };
        test_fn(&ctx)
    }
    
    fn test_concurrent<F>(&mut self, test_fn: F)
    where F: FnOnce(&ConcurrentQueueContext)
    {
        self.test_with(|ctx| {
            let concurrent_ctx = ConcurrentQueueContext::new(ctx);
            test_fn(&concurrent_ctx);
        });
    }
}

struct OpCodeQueueContext<'a> {
    queue: &'a OpCodeQueue,
    storage: &'a MockStorage,
}

struct ConcurrentQueueContext<'a> {
    ctx: &'a OpCodeQueueContext<'a>,
}

impl<'a> ConcurrentQueueContext<'a> {
    fn spawn_producer(&self, count: usize, prefix: &str) -> JoinHandle<()> {
        let queue = self.ctx.queue.clone();
        let prefix = prefix.to_string();
        
        thread::spawn(move || {
            for i in 0..count {
                let op = OpCode::new(
                    OpType::Write, 
                    &format!("{}/file{}.txt", prefix, i), 
                    vec![i as u8; 1024]
                );
                queue.enqueue(op).unwrap();
            }
        })
    }
    
    fn verify_all_processed(&self, expected_count: usize) {
        // Wait for processing and verify
        thread::sleep(Duration::from_millis(200));
        let processed = self.ctx.storage.get_operation_count();
        assert_eq!(processed, expected_count);
    }
}
```

## Usage Examples

### Simple File Operations Test

```rust
#[test]
fn test_file_create_and_read() {
    let mut harness = FilesystemTestHarness::new().unwrap();
    
    harness.test_file_ops(|file_ops| {
        // Test focuses only on the behavior
        file_ops.create_file("test.txt", "Hello, world!").unwrap();
        
        let content = file_ops.read_file("test.txt").unwrap();
        assert_eq!(content, "Hello, world!");
        
        // Verify it appears in source directory
        file_ops.verify_in_source("test.txt", "Hello, world!").unwrap();
    });
}
```

### Mount-Specific Test

```rust
#[test]
fn test_mounted_filesystem_operations() {
    let mut harness = FilesystemTestHarness::new()
        .unwrap()
        .with_mount()
        .unwrap();
    
    harness.test_file_ops(|file_ops| {
        file_ops.create_file("mounted_test.txt", "Mounted content").unwrap();
        file_ops.verify_in_source("mounted_test.txt", "Mounted content").unwrap();
    });
}
```

### Concurrent OpCode Queue Test

```rust
#[test]
fn test_concurrent_opcode_processing() {
    let mut harness = OpCodeQueueHarness::new().with_processor();
    
    harness.test_concurrent(|concurrent| {
        // Spawn multiple producers
        let handles: Vec<_> = (0..5).map(|i| {
            concurrent.spawn_producer(10, &format!("thread{}", i))
        }).collect();
        
        // Wait for all producers
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Verify all operations were processed
        concurrent.verify_all_processed(50);
    });
}
```

### Property-Based Testing Integration

```rust
#[test]
fn test_path_handling_properties() {
    let mut harness = PathManagerHarness::new();
    
    harness.test_with_properties(|path_ctx| {
        proptest!(|(paths in prop::collection::vec(any::<String>(), 1..100))| {
            path_ctx.test_path_roundtrip(&paths);
            path_ctx.test_inode_uniqueness(&paths);
        });
    });
}
```

## Benefits of This Approach

### 1. **No Duplication**
- Setup code written once per harness type
- Test functions focus purely on behavior
- Cleanup happens automatically

### 2. **Composable**
- Harnesses can be chained with builder pattern
- Different test contexts for different scenarios
- Easy to add new test types

### 3. **Type Safety**
- Context structs provide safe APIs
- Compiler prevents invalid operations
- Clear separation of concerns

### 4. **Fast & Reliable**
- No timing dependencies in test functions
- Mocked components where appropriate
- Deterministic test execution

### 5. **Easy to Debug**
- Clear error propagation
- Focused test scope
- Rich context for test failures

## Mock Components

```rust
#[derive(Clone)]
struct MockStorage {
    operations: Arc<Mutex<Vec<OpCode>>>,
    fail_pattern: Option<String>,
}

impl MockStorage {
    fn new() -> Self {
        Self {
            operations: Arc::new(Mutex::new(Vec::new())),
            fail_pattern: None,
        }
    }
    
    fn with_failure_pattern(mut self, pattern: &str) -> Self {
        self.fail_pattern = Some(pattern.to_string());
        self
    }
    
    fn get_operation_count(&self) -> usize {
        self.operations.lock().unwrap().len()
    }
    
    fn get_operations_for_path(&self, path: &str) -> Vec<OpCode> {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .filter(|op| op.path == path)
            .cloned()
            .collect()
    }
}

impl Storage for MockStorage {
    fn store_opcode(&mut self, op: &OpCode) -> Result<(), String> {
        if let Some(ref pattern) = self.fail_pattern {
            if op.path.contains(pattern) {
                return Err("Simulated storage failure".to_string());
            }
        }
        
        self.operations.lock().unwrap().push(op.clone());
        Ok(())
    }
}
```

## Implementation Plan

### Phase 1: Core Harnesses (Week 1)
1. `FilesystemTestHarness` - Basic file operations
2. `OpCodeQueueHarness` - Queue mechanics 
3. `StorageHarness` - Storage backend testing

### Phase 2: Specialized Contexts (Week 2)
1. `FileOpsContext` - File-specific operations
2. `DirectoryOpsContext` - Directory operations
3. `ConcurrentQueueContext` - Thread safety testing

### Phase 3: Integration (Week 2)
1. Property-based testing integration
2. Mock component library
3. Performance testing harnesses

This framework transforms chaotic, duplicate test code into a clean, maintainable system where tests are focused, fast, and reliable.