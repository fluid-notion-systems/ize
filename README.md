# Ize

A version-controlled filesystem implemented with FUSE in Rust.

## What is Ize?

Ize tracks every change to your files automatically, like Git but at the filesystem level. Mount a directory, and all file operations are transparently versioned - no commits needed.

## Quick Start

### Prerequisites

- Rust toolchain (2021 edition or newer)
- libfuse3 development package:
  - Ubuntu/Debian: `sudo apt install libfuse3-dev`
  - Fedora: `sudo dnf install fuse3-devel`
  - Arch Linux: `sudo pacman -S fuse3`

### Building

```bash
git clone https://github.com/fluid-notion-systems/ize.git
cd ize
cargo build --release
```

### Basic Usage

```bash
# Initialize a directory for version control
ize init /path/to/directory

# Mount the filesystem
ize mount /path/to/directory /mount/point

# Use the mounted filesystem normally - all changes are tracked!

# Unmount when done
fusermount -u /mount/point
```

### Advanced: fd-based Passthrough Mount

The `ize-mount-fd` binary provides an fd-based FUSE passthrough filesystem that eliminates re-entry deadlocks by opening the directory file descriptor before mounting:

```bash
# Build the binary
cargo build --release --bin ize-mount-fd

# Mount a directory with fd-based passthrough
# The directory is both the mount point and backing store
./target/release/ize-mount-fd /path/to/directory

# Mount in read-only mode
./target/release/ize-mount-fd /path/to/directory --read-only

# Enable debug logging
./target/release/ize-mount-fd /path/to/directory --log-level debug

# Press Ctrl+C to unmount and exit
```

**Features:**
- **No re-entry deadlock**: Directory fd opened before FUSE mount
- **VCS detection**: Automatically detects `.git`, `.jj`, `.pijul` directories
- **Clean signal handling**: Ctrl+C gracefully unmounts
- **Backing filesystem trait**: Uses `BackingFs` abstraction with `*at()` syscalls

This is useful for development and testing of the core passthrough layer without the full versioning system.

## Documentation

- **[Architecture](architecture.md)** - System design, components, and roadmap
- **[Research](research/)** - Design documents and analysis
- **[VCS Filtering](crates/ize-lib/src/vcs/README.md)** - VCS backend system documentation
- **[Contributing](CONTRIBUTING.md)** - Development guidelines

## Status

Currently in Phase 1: Building a robust testing framework and benchmarking system. Core passthrough filesystem is operational.

## License

MIT
