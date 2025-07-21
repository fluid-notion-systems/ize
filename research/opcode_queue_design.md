# OpCode Queue Design for Ize

## Overview

The OpCode queue system is the heart of Ize's performance architecture. It decouples filesystem operations from persistence, allowing the FUSE layer to respond immediately while changes are asynchronously persisted in the background.

## Current State Analysis

### Problems with Command-based Architecture
1. **Naming Confusion**: "Command" suggests imperative actions, but we're storing **completed operations**
2. **Heavy Structures**: Current `Command` struct carries too much metadata
3. **Synchronous Bottlenecks**: Some operations still block on database writes
4. **Memory Overhead**: Large payloads stored in memory queue indefinitely

### What We're Really Storing
- **OpCodes**: Completed filesystem operations with their effects
- **Deltas**: Changes made to files/directories
- **Metadata**: Timestamps, permissions, sizes at operation time
- **Content**: Actual file data for create/write operations

## OpCode Architecture Design

### Core OpCode Structure

```rust
/// Represents a completed filesystem operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpCode {
    /// Unique identifier (None until persisted)
    pub id: Option<u64>,

    /// Type of operation performed
    pub op_type: OpType,

    /// When the operation occurred (Unix timestamp)
    pub timestamp: u64,

    /// Primary path affected by the operation
    pub path: PathBuf,

    /// Secondary path for operations like rename
    pub target_path: Option<PathBuf>,

    /// Operation-specific data
    pub data: OpData,

    /// File/directory metadata at operation time
    pub metadata: FileMetadata,

    /// Optional link to related operations
    pub parent_id: Option<u64>,
}

/// Types of filesystem operations we track
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpType {
    // File operations
    FileCreate,
    FileWrite { offset: u64, size: u64 },
    FileDelete,
    FileTruncate { new_size: u64 },

    // Directory operations
    DirCreate,
    DirDelete,

    // Movement operations
    Rename { from: PathBuf, to: PathBuf },

    // Metadata operations
    Chmod { old_mode: u32, new_mode: u32 },
    Chown { old_uid: u32, old_gid: u32, new_uid: u32, new_gid: u32 },
    Touch { atime: u64, mtime: u64 },

    // Special file operations
    Symlink { target: PathBuf },
    Hardlink { target: PathBuf },
}

/// Operation-specific data payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpData {
    /// No additional data needed
    None,

    /// File content (for creates/writes)
    Content(Vec<u8>),

    /// Content reference (for large files)
    ContentRef {
        hash: String,
        size: u64,
        storage_path: PathBuf
    },

    /// Directory listing (for directory operations)
    DirectoryListing(Vec<String>),

    /// Extended attributes
    XAttrs(HashMap<String, Vec<u8>>),
}

/// Compact metadata representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}
```

### OpCode Queue Implementation

```rust
/// High-performance, thread-safe operation queue
pub struct OpCodeQueue {
    /// In-memory queue for fast enqueue/dequeue
    queue: Arc<Mutex<VecDeque<OpCode>>>,

    /// Maximum queue size before backpressure
    max_size: usize,

    /// Current queue statistics
    stats: Arc<Mutex<QueueStats>>,

    /// Notification system for queue events
    notify: Arc<Notify>,
}

impl OpCodeQueue {
    pub fn new(max_size: usize) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::with_capacity(max_size))),
            max_size,
            stats: Arc::new(Mutex::new(QueueStats::default())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Enqueue operation with backpressure handling
    pub async fn enqueue(&self, opcode: OpCode) -> Result<(), QueueError> {
        // Check queue size for backpressure
        let queue_len = {
            let queue = self.queue.lock().await;
            queue.len()
        };

        if queue_len >= self.max_size {
            // Apply backpressure - could return error or wait
            return Err(QueueError::QueueFull);
        }

        // Optimize large content by moving to staging area
        let optimized_opcode = self.optimize_opcode(opcode).await?;

        {
            let mut queue = self.queue.lock().await;
            queue.push_back(optimized_opcode);
        }

        // Update stats and notify consumers
        self.update_stats(1, 0).await;
        self.notify.notify_one();

        Ok(())
    }

    /// Dequeue batch of operations for processing
    pub async fn dequeue_batch(&self, max_batch_size: usize) -> Vec<OpCode> {
        let mut queue = self.queue.lock().await;
        let batch_size = queue.len().min(max_batch_size);

        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            if let Some(opcode) = queue.pop_front() {
                batch.push(opcode);
            }
        }

        // Update stats
        if !batch.is_empty() {
            self.update_stats(0, batch.len()).await;
        }

        batch
    }

    /// Optimize OpCode for memory efficiency
    async fn optimize_opcode(&self, mut opcode: OpCode) -> Result<OpCode, QueueError> {
        const LARGE_CONTENT_THRESHOLD: usize = 64 * 1024; // 64KB

        match &opcode.data {
            OpData::Content(content) if content.len() > LARGE_CONTENT_THRESHOLD => {
                // Move large content to staging area
                let hash = self.compute_content_hash(content);
                let staging_path = self.create_staging_file(&hash, content).await?;

                opcode.data = OpData::ContentRef {
                    hash,
                    size: content.len() as u64,
                    storage_path: staging_path,
                };
            }
            _ => {} // Keep small content in memory
        }

        Ok(opcode)
    }

    async fn update_stats(&self, enqueued: usize, dequeued: usize) {
        let mut stats = self.stats.lock().await;
        stats.total_enqueued += enqueued;
        stats.total_dequeued += dequeued;
        stats.current_size = stats.total_enqueued - stats.total_dequeued;
    }
}
```

### Background Processing Pipeline

```rust
/// Processes OpCodes from queue and persists to storage
pub struct OpCodeProcessor {
    /// Reference to the shared queue
    queue: Arc<OpCodeQueue>,

    /// Storage backend for persistence
    storage: Arc<dyn Storage + Send + Sync>,

    /// Processing configuration
    config: ProcessorConfig,

    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    /// How often to check for new operations
    pub poll_interval: Duration,

    /// Maximum batch size for processing
    pub max_batch_size: usize,

    /// Number of worker threads
    pub worker_threads: usize,

    /// Whether to compress operations before storage
    pub enable_compression: bool,
}

impl OpCodeProcessor {
    pub fn new(
        queue: Arc<OpCodeQueue>,
        storage: Arc<dyn Storage + Send + Sync>,
        config: ProcessorConfig,
    ) -> Self {
        Self {
            queue,
            storage,
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start background processing
    pub fn start(&self) -> JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let storage = Arc::clone(&self.storage);
        let config = self.config.clone();
        let shutdown = Arc::clone(&self.shutdown);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(config.poll_interval);

            while !shutdown.load(Ordering::Relaxed) {
                interval.tick().await;

                // Process available operations
                let batch = queue.dequeue_batch(config.max_batch_size).await;
                if !batch.is_empty() {
                    if let Err(e) = Self::process_batch(&*storage, batch, &config).await {
                        eprintln!("Error processing OpCode batch: {}", e);
                    }
                }
            }
        })
    }

    async fn process_batch(
        storage: &dyn Storage,
        batch: Vec<OpCode>,
        config: &ProcessorConfig,
    ) -> Result<(), ProcessingError> {
        // Process batch with optional compression
        let processed_batch = if config.enable_compression {
            Self::compress_batch(batch)?
        } else {
            batch
        };

        // Store operations in transaction
        storage.store_batch(&processed_batch).await?;

        Ok(())
    }

    fn compress_batch(batch: Vec<OpCode>) -> Result<Vec<OpCode>, ProcessingError> {
        // Implement operation compression/deduplication
        // For example, multiple writes to same file can be merged
        Ok(batch) // Placeholder
    }
}
```

## Advanced Features

### Operation Coalescing

```rust
/// Combines related operations for efficiency
pub struct OpCodeCoalescer {
    /// Buffer for collecting operations
    buffer: HashMap<PathBuf, Vec<OpCode>>,

    /// When to flush the buffer
    flush_threshold: Duration,

    /// Last flush time
    last_flush: Instant,
}

impl OpCodeCoalescer {
    /// Add operation to coalescing buffer
    pub fn add_operation(&mut self, opcode: OpCode) -> Vec<OpCode> {
        let path = opcode.path.clone();
        self.buffer.entry(path).or_default().push(opcode);

        // Check if we should flush
        if self.should_flush() {
            self.flush()
        } else {
            Vec::new()
        }
    }

    fn should_flush(&self) -> bool {
        self.last_flush.elapsed() > self.flush_threshold
            || self.buffer.len() > 1000 // Size-based flush
    }

    fn flush(&mut self) -> Vec<OpCode> {
        let mut result = Vec::new();

        for (path, operations) in self.buffer.drain() {
            // Coalesce operations on same path
            let coalesced = self.coalesce_operations(operations);
            result.extend(coalesced);
        }

        self.last_flush = Instant::now();
        result
    }

    fn coalesce_operations(&self, mut operations: Vec<OpCode>) -> Vec<OpCode> {
        if operations.len() <= 1 {
            return operations;
        }

        // Sort by timestamp
        operations.sort_by_key(|op| op.timestamp);

        let mut result = Vec::new();
        let mut current = operations.into_iter();

        if let Some(mut base) = current.next() {
            for next in current {
                match (&base.op_type, &next.op_type) {
                    // Multiple writes can be coalesced into final write
                    (OpType::FileWrite { .. }, OpType::FileWrite { .. }) => {
                        base = next; // Keep latest write
                    }
                    // Create followed by write can be merged
                    (OpType::FileCreate, OpType::FileWrite { .. }) => {
                        base.op_type = OpType::FileCreate;
                        base.data = next.data; // Use write content
                        base.timestamp = next.timestamp; // Use latest timestamp
                    }
                    _ => {
                        result.push(base);
                        base = next;
                    }
                }
            }
            result.push(base);
        }

        result
    }
}
```

### Performance Monitoring

```rust
#[derive(Debug, Default)]
pub struct QueueStats {
    pub total_enqueued: usize,
    pub total_dequeued: usize,
    pub current_size: usize,
    pub peak_size: usize,
    pub total_bytes_processed: u64,
    pub operations_per_second: f64,
    pub average_batch_size: f64,
}

impl QueueStats {
    pub fn queue_utilization(&self) -> f64 {
        self.current_size as f64 / self.peak_size.max(1) as f64
    }

    pub fn throughput_mbps(&self) -> f64 {
        (self.total_bytes_processed as f64 / 1_000_000.0) /
        (self.total_dequeued.max(1) as f64 / self.operations_per_second.max(0.1))
    }
}
```

## Integration with FUSE Layer

### FUSE Operation Capture

```rust
impl Filesystem for VersionedFS {
    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // Get path from inode
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Perform actual filesystem operation first
        let result = self.passthrough_fs.write(
            _req, ino, fh, offset, data, _write_flags, _flags, _lock_owner, reply
        );

        // If successful, create OpCode for background processing
        if result.is_ok() {
            let opcode = OpCode {
                id: None,
                op_type: OpType::FileWrite {
                    offset: offset as u64,
                    size: data.len() as u64
                },
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                path,
                target_path: None,
                data: OpData::Content(data.to_vec()),
                metadata: self.get_current_metadata(&path).unwrap_or_default(),
                parent_id: None,
            };

            // Enqueue for background processing (non-blocking)
            if let Err(e) = self.opcode_queue.try_enqueue(opcode) {
                // Log error but don't fail the filesystem operation
                eprintln!("Failed to enqueue OpCode: {}", e);
            }
        }
    }
}
```

## Benefits of OpCode Architecture

### 1. **Performance**
- Non-blocking filesystem operations
- Batched persistence for efficiency
- Memory optimization for large files
- Operation coalescing reduces storage overhead

### 2. **Reliability**
- Completed operations are never lost
- Graceful degradation under load
- Backpressure prevents memory exhaustion
- Crash recovery from persisted OpCodes

### 3. **Observability**
- Rich metrics and monitoring
- Operation tracing and debugging
- Performance profiling capabilities
- Queue health monitoring

### 4. **Scalability**
- Configurable worker threads
- Adaptive batch sizing
- Memory-efficient large file handling
- Horizontal scaling potential

This OpCode queue design provides the foundation for a high-performance, reliable version control filesystem that can handle real-world workloads while maintaining data integrity.
