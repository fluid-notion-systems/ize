# Command Structure System for Claris-FUSE

## File Access Types Analysis

Based on the PassthroughFS implementation, the following file operations need to be captured in the Command Queue system:

### 1. Core Write Operations

1. **Create** (`fn create`)
   - Creates new files in the filesystem
   - Needs to track initial file metadata and content

2. **Write** (`fn write`)
   - Modifies file content
   - Needs to track offsets, data written, and file size changes

3. **Truncate** (handled in `fn setattr`)
   - Changes file size (usually making it smaller)
   - Needs to track the new size and potentially store truncated data

4. **Unlink** (`fn unlink`)
   - Deletes files
   - Needs to store the deleted file content for potential restoration

5. **Rename** (`fn rename`)
   - Moves/renames files
   - Needs to track old and new paths

### 2. Directory Operations

1. **Mkdir** (`fn mkdir`)
   - Creates new directories
   - Needs to track directory metadata

2. **Rmdir** (`fn rmdir`)
   - Removes directories
   - Needs to track directory state before removal

### 3. Metadata Operations

1. **Chmod** (part of `fn setattr`)
   - Changes file permissions
   - Tracks permission changes only

2. **Chown** (part of `fn setattr`)
   - Changes file ownership
   - Tracks ownership changes only

3. **Utimens** (part of `fn setattr`)
   - Changes file timestamps
   - Tracks timestamp modifications

### 4. Special Operations

1. **Mknod** (`fn mknod`)
   - Creates special files (devices, named pipes)
   - Needs to track device information

2. **Symlink/Link** (not explicitly seen but would be implemented)
   - Creates symbolic or hard links
   - Needs to track link targets and relationships

## Command Structure Design

```rust
/// Represents the type of filesystem operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandType {
    // File operations
    Create,
    Write,
    Truncate,
    Unlink,
    Rename,
    
    // Directory operations
    Mkdir,
    Rmdir,
    
    // Metadata operations
    Chmod,
    Chown,
    Utimens,
    
    // Special operations
    Mknod,
    Symlink,
    Link,
}

/// Represents a command in the queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    // Core identification
    id: Option<u64>,                // Optional ID (None until persisted)
    command_type: CommandType,      // Type of operation
    timestamp: u64,                 // When the operation occurred
    
    // Path information
    path: String,                   // Primary path affected
    secondary_path: Option<String>, // For operations like rename that affect two paths
    
    // File content
    content: Option<Vec<u8>>,       // File content (for operations that modify content)
    content_offset: Option<u64>,    // Offset for write operations
    content_size: Option<u64>,      // Size for truncate operations
    
    // Metadata
    metadata: FileMetadata,         // File/directory metadata
    
    // Relationships
    parent_command_id: Option<u64>, // Optional link to parent command
}

/// File metadata representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    mode: u32,                     // File mode/permissions
    uid: u32,                      // User ID
    gid: u32,                      // Group ID
    size: u64,                     // File size
    atime: u64,                    // Access time
    mtime: u64,                    // Modification time
    ctime: u64,                    // Change time
}
```

## Command Queue Implementation

```rust
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

/// Thread-safe command queue for asynchronous processing
pub struct CommandQueue {
    queue: Arc<Mutex<VecDeque<Command>>>,
    max_batch_size: usize,
}

impl CommandQueue {
    /// Create a new command queue
    pub fn new(max_batch_size: usize) -> Self {
        CommandQueue {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            max_batch_size,
        }
    }
    
    /// Add a command to the queue
    pub fn enqueue(&self, command: Command) -> Result<(), String> {
        match self.queue.lock() {
            Ok(mut queue) => {
                queue.push_back(command);
                Ok(())
            },
            Err(e) => Err(format!("Failed to acquire lock: {}", e)),
        }
    }
    
    /// Get a batch of commands from the queue
    pub fn dequeue_batch(&self) -> Result<Vec<Command>, String> {
        match self.queue.lock() {
            Ok(mut queue) => {
                let mut batch = Vec::with_capacity(self.max_batch_size);
                for _ in 0..self.max_batch_size {
                    if let Some(cmd) = queue.pop_front() {
                        batch.push(cmd);
                    } else {
                        break;
                    }
                }
                Ok(batch)
            },
            Err(e) => Err(format!("Failed to acquire lock: {}", e)),
        }
    }
    
    /// Get the shared queue reference for worker threads
    pub fn get_shared_queue(&self) -> Arc<Mutex<VecDeque<Command>>> {
        Arc::clone(&self.queue)
    }
    
    /// Get the current queue length
    pub fn len(&self) -> Result<usize, String> {
        match self.queue.lock() {
            Ok(queue) => Ok(queue.len()),
            Err(e) => Err(format!("Failed to acquire lock: {}", e)),
        }
    }
    
    /// Check if the queue is empty
    pub fn is_empty(&self) -> Result<bool, String> {
        match self.queue.lock() {
            Ok(queue) => Ok(queue.is_empty()),
            Err(e) => Err(format!("Failed to acquire lock: {}", e)),
        }
    }
}
```

## Command Processor Implementation

```rust
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

/// Processes commands from the queue and persists them to storage
pub struct CommandProcessor {
    queue: Arc<Mutex<VecDeque<Command>>>,
    storage: Arc<Mutex<dyn Storage>>,
    running: Arc<AtomicBool>,
    processing_interval_ms: u64,
}

impl CommandProcessor {
    /// Create a new command processor
    pub fn new(
        queue: Arc<Mutex<VecDeque<Command>>>,
        storage: Arc<Mutex<dyn Storage>>,
        processing_interval_ms: u64
    ) -> Self {
        CommandProcessor {
            queue,
            storage,
            running: Arc::new(AtomicBool::new(false)),
            processing_interval_ms,
        }
    }
    
    /// Start the background processing thread
    pub fn start(&self) -> JoinHandle<()> {
        self.running.store(true, Ordering::SeqCst);
        
        let queue = Arc::clone(&self.queue);
        let storage = Arc::clone(&self.storage);
        let running = Arc::clone(&self.running);
        let interval = self.processing_interval_ms;
        
        thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                // Process any available commands
                Self::process_commands(&queue, &storage);
                
                // Sleep for a bit to avoid busy-waiting
                thread::sleep(Duration::from_millis(interval));
            }
        })
    }
    
    /// Stop the background processing thread
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
    
    /// Process commands from the queue
    fn process_commands(
        queue: &Arc<Mutex<VecDeque<Command>>>,
        storage: &Arc<Mutex<dyn Storage>>
    ) {
        // Get a batch of commands to process
        let commands = {
            let mut queue_guard = match queue.lock() {
                Ok(guard) => guard,
                Err(_) => return, // Failed to acquire lock, try again later
            };
            
            let mut batch = Vec::new();
            let max_batch_size = 10; // Process up to 10 commands at once
            
            for _ in 0..max_batch_size {
                if let Some(cmd) = queue_guard.pop_front() {
                    batch.push(cmd);
                } else {
                    break;
                }
            }
            
            batch
        };
        
        // If no commands, just return
        if commands.is_empty() {
            return;
        }
        
        // Process each command
        let mut storage_guard = match storage.lock() {
            Ok(guard) => guard,
            Err(_) => {
                // Failed to acquire storage lock, put commands back in queue
                let mut queue_guard = match queue.lock() {
                    Ok(guard) => guard,
                    Err(_) => return, // Both locks failed, give up for now
                };
                
                // Put commands back at the front of the queue
                for cmd in commands.into_iter().rev() {
                    queue_guard.push_front(cmd);
                }
                return;
            }
        };
        
        // Store each command in the database
        for cmd in commands {
            if let Err(e) = storage_guard.store_command(&cmd) {
                // Log the error
                eprintln!("Error storing command: {}", e);
                
                // Could implement retry logic here
            }
        }
    }
}
```

## Integration with PassthroughFS

### VersionedFS Structure

```rust
/// VersionedFS wraps PassthroughFS and adds version history tracking
pub struct VersionedFS {
    // The underlying passthrough filesystem
    passthrough: PassthroughFS,
    
    // Command queue for asynchronous processing
    command_queue: Arc<CommandQueue>,
    
    // Command processor for background processing
    command_processor: CommandProcessor,
    
    // Storage backend for persisting commands
    storage: Arc<Mutex<dyn Storage>>,
}

impl VersionedFS {
    /// Create a new versioned filesystem
    pub fn new(
        source_dir: &Path,
        mount_point: &Path,
        db_path: &Path,
        read_only: bool
    ) -> Result<Self, std::io::Error> {
        // Create the passthrough filesystem
        let passthrough = PassthroughFS::new(source_dir, mount_point, db_path, read_only)?;
        
        // Initialize storage
        let storage = Arc::new(Mutex::new(
            SqliteStorage::open(db_path)?
        ));
        
        // Create command queue
        let command_queue = Arc::new(CommandQueue::new(100)); // Process up to 100 commands at once
        
        // Create command processor
        let command_processor = CommandProcessor::new(
            command_queue.get_shared_queue(),
            Arc::clone(&storage),
            100 // Process every 100ms
        );
        
        // Start the command processor if not in read-only mode
        if !read_only {
            command_processor.start();
        }
        
        Ok(VersionedFS {
            passthrough,
            command_queue,
            command_processor,
            storage,
        })
    }
    
    /// Get the path for an inode (delegate to passthrough)
    fn get_path_for_inode(&self, ino: u64) -> Option<PathBuf> {
        self.passthrough.get_path_for_inode(ino)
    }
    
    /// Get file metadata (delegate to passthrough)
    fn get_file_metadata(&self, path: &Path) -> Result<FileMetadata, std::io::Error> {
        // This would need to be implemented to convert from filesystem metadata to our FileMetadata struct
        let attr = self.passthrough.getattr(path)?;
        
        Ok(FileMetadata {
            mode: attr.mode,
            uid: attr.uid,
            gid: attr.gid,
            size: attr.size,
            atime: attr.atime.as_secs(),
            mtime: attr.mtime.as_secs(),
            ctime: attr.ctime.as_secs(),
        })
    }
}
```

To integrate this Command Queue system with the existing PassthroughFS, we would need to:

1. Create a VersionedFS wrapper that contains PassthroughFS as a member variable
2. Initialize the CommandQueue and CommandProcessor in the VersionedFS constructor
3. Implement each filesystem operation to:
   - Create appropriate Command objects
   - Add them to the queue
   - Delegate to the PassthroughFS member for actual filesystem operations

For example, here's how the write operation might be wrapped:

```rust
impl Filesystem for VersionedFS {
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // Get the path for this inode
        let path = match self.passthrough.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        
        // Create a Write command
        let cmd = Command {
            id: None,
            command_type: CommandType::Write,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            path: path.to_string_lossy().to_string(),
            secondary_path: None,
            content: Some(data.to_vec()),
            content_offset: Some(offset as u64),
            content_size: None,
            metadata: self.passthrough.get_file_metadata(&path).unwrap_or_default(),
            parent_command_id: None,
        };
        
        // Add the command to the queue
        if let Err(e) = self.command_queue.enqueue(cmd) {
            // Log the error but continue with the operation
            eprintln!("Failed to enqueue write command: {}", e);
        }
        
        // Delegate to the passthrough filesystem to actually perform the write
        self.passthrough.write(
            _req,
            ino,
            fh,
            offset,
            data,
            _write_flags,
            _flags,
            _lock_owner,
            reply,
        )
    }
    
    // Similar implementations for other file operations...
}
```

## Summary of File Access Types to Support

1. **File Content Operations**:
   - Create: Creating new files
   - Write: Writing data to files at specific offsets
   - Truncate: Changing file size (usually shrinking)
   - Unlink: Deleting files

2. **Directory Operations**:
   - Mkdir: Creating directories
   - Rmdir: Removing directories
   - Rename: Moving files or directories
   - Readdir: Reading directory contents (might be tracked for access patterns)

3. **Metadata Operations**:
   - Chmod: Changing file permissions
   - Chown: Changing ownership
   - Utimens: Setting access and modification times
   - Setattr: General attribute setting (may include size changes)

4. **Special File Operations**:
   - Mknod: Creating device files
   - Symlink: Creating symbolic links
   - Link: Creating hard links

## Read-Only Operations (No Versioning Required)
1. **lookup** - Looking up directory entries
2. **getattr** - Getting file attributes
3. **open** - Opening files
4. **read** - Reading file data
5. **readdir** - Reading directory contents
6. **readlink** - Reading symbolic link targets
7. **access** - Checking file permissions
8. **getxattr** - Getting extended attributes
9. **listxattr** - Listing extended attributes
10. **flush/fsync/release** - Managing file handles