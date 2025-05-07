# Claris-FUSE: Version-Controlled Filesystem

## Project Overview
Claris-FUSE is a FUSE filesystem implementation in Rust that maintains a linear history of file operations (create/update/delete) similar to Git but at the filesystem level. It tracks changes to files over time, allowing users to view and restore previous versions.

## Technical Stack
- **Rust**: Programming language
- **Fuser (v0.15.1)**: A maintained fork of the fuse-rs crate that provides FUSE bindings for Rust
  - Benefits over fuse-rs:
    - Actively maintained (last commit: May 2025)
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

## Key Features (Planned)
1. Transparent versioning of all file operations
2. Ability to view file history
3. Restoring files to previous versions
4. History browsing through special filesystem interface
5. Configurable retention policies
6. AI-powered commit messages for file changes
7. Extended search capabilities through file history

## Implementation Approach
1. Use the fuser crate to implement core FUSE functionality
2. Intercept file operations (create, write, delete, etc.)
3. Store metadata about changes with timestamps
4. Define storage trait interface for version history
5. Implement SQLite backend as default storage engine
6. Provide API for browsing and restoring history
7. Allow for alternative storage backends to be implemented and plugged in
8. Integrate with LLM API for generating descriptive change summaries

## Usage Examples (Planned)
```bash
# Mount the filesystem
claris-fuse mount /path/to/source /mount/point

# View version history of a file
claris-fuse history /mount/point/file.txt

# Restore a file to a previous version
claris-fuse restore /mount/point/file.txt --version=3
```

The version history database `claris-fuse.db` will be stored in the source directory that is being mounted, allowing the version history to persist between different mount sessions.

## Development Status
- Initial research phase
- Selected fuser crate over fuse-rs
- Planning implementation details

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

## AI-Powered Change Descriptions

For meaningful file change descriptions:
1. Capture original and modified file content for each operation
2. Queue changes in background work queue for asynchronous processing
3. Process queue with dedicated worker threads to maintain filesystem performance
4. Send changes to LLM API with prompt requesting summary
5. Store generated descriptions with version metadata when they become available
6. Provide search capabilities across these descriptions
7. Allow filtering history based on semantic descriptions

The asynchronous approach ensures:
- Filesystem operations remain fast and responsive
- LLM API processing doesn't block file operations
- Descriptions are generated in background without impacting user experience
- System can handle high-volume changes without performance degradation
- Queue can be persisted to handle restarts/crashes

## Development Phases and Branching Strategy

Each phase of development will use a dedicated feature branch, with periodic merges to main after completing significant milestones. Each development step should have at least one dedicated commit with a clear, descriptive commit message.

### Phase 1: Foundation (`feature/foundation`)
1. Initialize Rust workspace (commit)
2. Add fuser dependency (commit)
3. Implement basic passthrough filesystem (1+ commits)
4. Design version history storage schema (commit)
5. Implement storage trait interface (commit)

### Phase 2: Core Functionality (`feature/core-versioning`)
1. Create SQLite storage backend (1+ commits)
2. Add versioning layer for core operations (multiple commits, one per operation type)
3. Create CLI tools for browsing and restoring history (1+ commits)
4. Write comprehensive tests (multiple commits, organized by component)

### Phase 3: Advanced Features (`feature/llm-integration`)
1. Implement async background processing system for LLM descriptions (1+ commits)
2. Integrate LLM API for change descriptions (1+ commits)
3. Add search capabilities across descriptions (commit)
4. Implement configurable retention policies (commit)

## Commit Guidelines

- Make atomic commits that focus on a single logical change
- Write descriptive commit messages explaining the "why" not just the "what"
- Include references to any research or decisions made
- Keep commits small and focused to make code review easier
- For large features, break them down into multiple sequential commits
