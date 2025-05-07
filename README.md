# Claris-FUSE

A version-controlled filesystem implemented with FUSE in Rust. Claris-FUSE maintains a linear history of file operations (create/update/delete) similar to Git but at the filesystem level.

## Features

- Transparent versioning of all file operations
- Ability to view file history
- Restoring files to previous versions 
- History browsing through special filesystem interface
- Configurable retention policies
- AI-powered commit messages for file changes (coming soon)
- Extended search capabilities through file history

## Prerequisites

- Rust toolchain (2021 edition or newer)
- libfuse3 development package
  - Ubuntu/Debian: `sudo apt install libfuse3-dev`
  - Fedora: `sudo dnf install fuse3-devel`
  - Arch Linux: `sudo pacman -S fuse3`

## Building

```bash
# Clone the repository
git clone https://github.com/claris/claris-fuse.git
cd claris-fuse

# Build the project
cargo build

# For release build
cargo build --release
```

## Usage

Currently, only the basic passthrough filesystem functionality is implemented (Phase 1).

```bash
# Mount a filesystem (read-only mode for now)
cargo run -- mount /path/to/source /mount/point

# With custom log level
cargo run -- --log-level debug mount /path/to/source /mount/point

# Using the binary directly (after building)
./target/debug/claris-fuse mount /path/to/source /mount/point

# For optimal performance, use the release build
./target/release/claris-fuse mount /path/to/source /mount/point
```

### Unmounting

To unmount the filesystem, use:

```bash
fusermount -u /mount/point
```

## Development Status

- [x] Phase 1: Foundation
  - [x] Basic passthrough filesystem
  - [x] Storage trait interface
  - [x] SQLite schema design

- [ ] Phase 2: Core Functionality
  - [ ] SQLite storage backend
  - [ ] Versioning layer
  - [ ] CLI tools for history and restore

- [ ] Phase 3: Advanced Features
  - [ ] Async background processing
  - [ ] LLM integration for change descriptions
  - [ ] Search capabilities
  - [ ] Configurable retention policies

## License

MIT