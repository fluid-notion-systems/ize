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
git clone https://github.com/fluid-notion-systems/Ize.git
cd Ize
cargo build --release
```

### Basic Usage

```bash
# Initialize a directory for version control
Ize init /path/to/directory

# Mount the filesystem
Ize mount /path/to/directory /mount/point

# Use the mounted filesystem normally - all changes are tracked!

# Unmount when done
fusermount -u /mount/point
```

## Documentation

- **[Architecture](architecture.md)** - System design, components, and roadmap
- **[Research](research/)** - Design documents and analysis
- **[Contributing](CONTRIBUTING.md)** - Development guidelines

## Status

Currently in Phase 1: Building a robust testing framework and benchmarking system. Core passthrough filesystem is operational.

## License

MIT
