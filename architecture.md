# Claris-FUSE: Version-Controlled Filesystem

## Project Overview
Claris-FUSE is a FUSE filesystem implementation in Rust that maintains a linear history of file operations (create/update/delete) similar to Git but at the filesystem level. It tracks changes to files over time, allowing users to view and restore previous versions.

## Technical Stack
- **Rust**: Programming language
- **Fuser (v0.15.1)**: A maintained fork of the fuse-rs crate that provides FUSE bindings for Rust
  - Benefits over fuse-rs:
    - Actively maintained (last commit: May 2023)
    - Support for newer FUSE ABIs
    - Better documentation and examples
    - File descriptor passthrough functionality
- **SQLite**: Initial storage backend for revision history
  - Simple to implement and maintain
  - Good performance for initial development
  - Database stored in source directory as `claris-fuse.db`
- **Pluggable Storage Engine**: Using Rust traits to support multiple backends
  - SQLite as default implementation
  - Interface for creating custom storage engines
  - Easy to swap implementations as performance needs change
- **Command Queue System**: Asynchronous processing of filesystem operations
  - In-memory queue for efficient operation handling
  - Background thread for database persistence
  - Maintains system performance while capturing history

### Key Features (Implemented and Planned)
1. **Implemented**:
   - Robust transparent filesystem passthrough
   - Path management with consistent handling of special cases
   - Support for complex applications in mounted filesystem
   - Database initialization and validation
   - Read-only mode support
   - Command-line interface for all operations

2. **Planned**:
   - Asynchronous command queue architecture for performance
   - Transparent versioning of all file operations
   - Ability to view file history
   - Restoring files to previous versions
   - History browsing through special filesystem interface
   - Configurable retention policies
   - AI-powered commit messages for file changes
   - Extended search capabilities through file history

## Implementation Approach
1. Use the fuser crate to implement core FUSE functionality with PassthroughFS
2. Intercept file operations (create, write, delete, etc.) and add to command queue
3. Process command queue asynchronously to store history in database
4. Define storage trait interface for version history and command persistence
5. Implement SQLite backend as default storage engine
6. Provide API for browsing and restoring history
7. Allow for alternative storage backends to be implemented and plugged in
8. Integrate with LLM API for generating descriptive change summaries

### Implementation Phases

#### Phase 1: Foundation (Completed)
- Implement PassthroughFS with robust file operations
- Create path management system for consistent handling
- Develop command-line interface for basic operations
- Set up database schema and storage interface

#### Phase 2: Command Queue System (Current)
- Design command representations for filesystem operations
- Implement thread-safe command queue with Arc<Mutex<Queue>>
- Create background worker thread for async processing
- Develop serialization/deserialization for commands

#### Phase 3: History Tracking
- Implement versioning layer on top of PassthroughFS
- Store command history in SQLite database
- Create APIs for retrieving version history
- Implement file restoration functionality

#### Phase 4: Advanced Features
- Implement delta storage for efficient history
- Add AI-powered commit messages for changes
- Develop extended search capabilities
- Create configurable retention policies

## Usage Examples
```bash
# Initialize a directory for version control (creates claris-fuse.db)
claris-fuse init /path/to/directory

# Mount the filesystem
# The first argument is the initialized directory, the second is the mount point
claris-fuse mount /path/to/initialized/directory /path/to/mount/point

# Mount in read-only mode
claris-fuse mount --read-only /path/to/initialized/directory /path/to/mount/point

# View version history of a file (planned)
claris-fuse history /path/to/file.txt

# Restore a file to a previous version (planned)
claris-fuse restore /path/to/file.txt --version=3
```

The version history database file (typically named `claris-fuse.db`) is created when initializing a directory for version control. The database file can be located anywhere on the filesystem except inside the mount point directory (this is checked and prevented to avoid recursion issues). When mounted, the database file will be hidden from the view in the mount point, even though other files in its directory will be visible.

The `mount` command requires two parameters: the path to the initialized directory and the path to the mount point. The content shown in the mount point will be from the directory containing the database file.

## System Architecture

### Command Queue Subsystem
The command queue subsystem is designed to efficiently capture filesystem operations while maintaining performance:

1. **Command Structure**: Represents filesystem operations:
   - Encapsulates operation type (write, create, delete, etc.)
   - Includes metadata (timestamps, file paths, etc.)
   - Serializable for storage

2. **Queue Manager**: Handles in-memory command processing:
   - Thread-safe queue using Arc<Mutex<Queue<Command>>>
   - Background worker thread for database persistence
   - Batching for efficient storage operations

3. **Command Consumer**: Processes commands asynchronously:
   - Reads from command queue
   - Persists changes to storage backend
   - Handles error recovery and retry logic

### Storage Subsystem
The storage subsystem is designed with a trait-based approach to allow for multiple backend implementations:

1. **Storage Trait**: Defines core operations for any storage engine:
   - `read` - Read data from storage
   - `write` - Write data to storage
   - `delete` - Delete data from storage
   - `store_command` - Store a command in the history

2. **StorageManager**: Static methods for initializing, validating, and opening storage:
   - `init` - Initialize a new database in the specified directory
   - `is_valid` - Check if a directory contains a valid database
   - `open` - Open an existing database for read/write operations

3. **SqliteStorage**: Default implementation using SQLite:
   - Manages database schema creation
   - Handles data storage and retrieval
   - Provides version history tracking

### Database Schema
The SQLite schema is designed to efficiently track file and directory history:

1. **Commands**:
   - `id`: BigInt (Primary Key)
   - `command_type`: Text (e.g., "write", "create", "delete")
   - `path`: Text (Relative path of affected file/directory)
   - `timestamp`: BigInt (Unix timestamp)
   - `parent_command_id`: BigInt (Optional, for linking related commands)
   - `metadata`: Text (JSON-serialized command metadata)
   - `content_id`: BigInt (Foreign Key to Content, if applicable)

2. **Directories**:
   - `id`: BigInt (Primary Key)
   - `path`: Text (Unique)
   - `created_at`: BigInt (Unix timestamp)
   - `metadata_id`: Foreign Key to Metadata

3. **Files**:
   - `id`: BigInt (Primary Key)
   - `directory_id`: BigInt (Foreign Key to Directory)
   - `name`: Text
   - `created_at`: BigInt (Unix timestamp)
   - `metadata_id`: BigInt (Foreign Key to Metadata)

4. **Metadata** (shared between files and directories):
   - `id`: BigInt (Primary Key)
   - `mode`: Integer (File permissions)
   - `uid`: Integer (User ID)
   - `gid`: Integer (Group ID)
   - `atime`: BigInt (Access time)
   - `mtime`: BigInt (Modification time)
   - `ctime`: BigInt (Change time - when metadata or content was last changed)

5. **Content**:
   - `id`: BigInt (Primary Key)
   - `file_id`: BigInt (Foreign Key to File)
   - `data`: Binary (The actual file content as raw bytes)

### Filesystem Implementation
The implementation consists of two main layers:

1. **PassthroughFS Layer**:
   - Transparent file and directory operations
   - Read-only mode support
   - Database path validation
   - Hiding the database file from the mounted view
   - Path management for consistent handling of special cases

2. **VersionedFS Layer**:
   - Wraps PassthroughFS to intercept operations
   - Queues commands for asynchronous processing
   - Delegates actual filesystem operations to PassthroughFS
   - Minimal performance impact on filesystem operations

3. **Command Processing Layer**:
   - Background thread for processing command queue
   - Batches commands for efficient storage
   - Handles persistence to database
   - Manages error recovery and retry logic

### Command Line Interface
The CLI is implemented using clap with the following commands:

1. **init**: Initialize a directory for version control
   - Creates a new SQLite database
   - Sets up initial schema and root directory

2. **mount**: Mount a version-controlled filesystem
   - Verifies the directory is properly initialized
   - Supports read-only mode
   - Option to unmount on program exit

3. **history**: View file version history
   - Show changes made to a file over time
   - Support for limiting number of versions displayed
   - Verbose mode for detailed information

4. **restore**: Restore file to a previous version
   - Restore specific file version
   - Force option to skip confirmation

## Development Status

### Completed
- Robust FUSE passthrough filesystem implementation
  - Support for standard file operations (read, write, create, delete)
  - Directory creation and removal
  - File renaming and moving
  - Complex application support (tested with Zed editor)
- Advanced path management with consistent handling
- Database schema design
- Storage trait interface and SQLite implementation
- Directory initialization command
- Mount command with validation and read-only support
- Signal handling for clean unmounting

### In Progress
- Designing command queue architecture for asynchronous history tracking
- Creating command representations for various filesystem operations
- Implementing thread-safe queue with background worker thread
- Developing efficient serialization/deserialization of filesystem commands
- Optimizing performance for large files and directories

### Planned
- Version tracking layer implementation on top of passthrough
- History command implementation
- File restoration functionality
- Delta storage for efficient version history
- Extended search capabilities
- Configuration options for retention policies
- LLM integration for semantic change descriptions

## File Operations to Support

The following FUSE filesystem operations will need to be implemented and tracked for version history:

### Core File Operations (Version Tracked)
1. **create** - Creating new files
2. **write** - Modifying file content
3. **unlink** - Deleting files
4. **rename** - Moving/renaming files
5. **truncate** - Resizing files (via setattr)
6. **mkdir** - Creating directories
7. **rmdir** - Removing directories
8. **symlink** - Creating symbolic links
9. **link** - Creating hard links
10. **setattr** - Setting file attributes (especially when changing size)

### Additional Operations (Metadata Only)
1. **chmod** - Changing permissions
2. **chown** - Changing ownership
3. **utimens** - Changing timestamps
4. **setxattr** - Setting extended attributes
5. **removexattr** - Removing extended attributes

### Read-Only Operations (No Versioning Required)
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

## Future Enhancements

### Command Queue Optimization
For high-performance filesystem history tracking:
1. Implement batching of similar operations
2. Add prioritization for different command types
3. Develop adaptive persistence strategies based on system load
4. Implement command compression for large batches
5. Add conflict resolution for overlapping operations

### AI-Powered Change Descriptions
For meaningful file change descriptions:
1. Capture original and modified file content for each operation
2. Queue changes in background work queue for asynchronous processing
3. Process queue with dedicated worker threads to maintain filesystem performance
4. Send changes to LLM API with prompt requesting summary
5. Store generated descriptions with version metadata when they become available
6. Provide search capabilities across these descriptions
7. Allow filtering history based on semantic descriptions

### Delta Storage
To optimize storage use for large files:
1. Store initial file content in full
2. For subsequent changes, store binary deltas
3. Calculate deltas using efficient diff algorithms
4. Provide configuration for compression and retention policies
5. Support pruning of historical versions based on age or space constraints

## Development Workflow & Practices

### Commit Guidelines

- Make atomic commits that focus on a single logical change
- Write descriptive commit messages explaining the "why" not just the "what"
- Include references to any research or decisions made
- Keep commits small and focused to make code review easier
- For large features, break them down into multiple sequential commits

### Code Quality Tools

#### Husky and Pre-commit Hooks

The project uses Husky to manage Git hooks, particularly pre-commit hooks, which:
- Run Clippy (Rust linter) to catch common mistakes and enforce code style
- Run the test suite to ensure nothing breaks
- Format code using rustfmt to maintain consistent styling
- Validate commit messages against project standards

This automated validation ensures that all committed code meets the project's quality standards before it reaches the repository.
