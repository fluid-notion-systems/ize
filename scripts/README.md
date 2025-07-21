# Ize Nu Shell Scripts

This directory contains Nu shell scripts for development workflow automation.

## Prerequisites

- [Nu Shell](https://www.nushell.sh/) (latest version from git)
- cargo-watch: `cargo install cargo-watch`
- (Optional) cargo-nextest: `cargo install cargo-nextest`

## Scripts

### `dev.nu` - Main Development Tools

A comprehensive development script with multiple commands:

```nu
# Watch files and run appropriate commands
nu scripts/dev.nu watch        # Smart mode (default)
nu scripts/dev.nu watch -t     # Test only mode
nu scripts/dev.nu watch -b     # Build only mode
nu scripts/dev.nu watch -a     # All checks mode

# Run specific tasks
nu scripts/dev.nu test         # Run tests
nu scripts/dev.nu build        # Build project
nu scripts/dev.nu clippy       # Run clippy
nu scripts/dev.nu fmt          # Format code
nu scripts/dev.nu check        # Run all checks
nu scripts/dev.nu clean        # Clean artifacts
nu scripts/dev.nu stats        # Show project statistics
```

### `watch.nu` - Simple Watch Script

A focused watch script with cargo-watch integration:

```nu
# Default smart watch mode
nu scripts/watch.nu

# Specific modes
nu scripts/watch.nu test       # Test watch only
nu scripts/watch.nu build      # Build watch only
nu scripts/watch.nu alt        # Use Nu's built-in watch
```

### `w.nu` - Quick Launcher

The simplest way to start watching:

```nu
nu scripts/w.nu                # Start smart watch mode
```

## Usage Examples

### From Project Root

```bash
# Start watching (smart mode)
nu scripts/w.nu

# Run all checks
nu scripts/dev.nu check

# Watch and run tests only
nu scripts/dev.nu watch --test-only

# Build in release mode
nu scripts/dev.nu build --release

# Run specific test
nu scripts/dev.nu test my_test_name
```

### Setting Up Aliases

Add to your Nu config (`$nu.config-path`):

```nu
# Quick development commands
alias dw = nu ~/path/to/Ize/scripts/w.nu
alias dt = nu ~/path/to/Ize/scripts/dev.nu test
alias db = nu ~/path/to/Ize/scripts/dev.nu build
alias dc = nu ~/path/to/Ize/scripts/dev.nu check
```

## Watch Modes

### Smart Mode (Default)
- Tests run when files in `tests/` change
- Build runs when files in `src/` change
- Both run when `Cargo.toml` changes

### Test-Only Mode (`-t`, `--test-only`)
- Only runs `cargo test` on any change

### Build-Only Mode (`-b`, `--build-only`)
- Only runs `cargo build` on any change

### All Mode (`-a`, `--all`)
- Runs full check suite on any change: format → clippy → test → build

## Features

- **Color Output**: Clear visual feedback with color-coded messages
- **Smart Detection**: Automatically runs appropriate commands based on changed files
- **Project Statistics**: View code metrics with `dev stats`
- **Tool Checking**: Automatically checks for and offers to install missing tools
- **Flexible Testing**: Support for cargo-nextest and test filtering
- **Watch Options**: Multiple watch modes for different workflows

## Tips

1. **Quick Start**: Just run `nu scripts/w.nu` from anywhere in the project
2. **Test Filtering**: Use `nu scripts/dev.nu test pattern` to run specific tests
3. **Feature Flags**: Build with features using `nu scripts/dev.nu build -f "feature1,feature2"`
4. **Deep Clean**: Use `nu scripts/dev.nu clean -d` to remove all build artifacts

## Troubleshooting

### Scripts Not Found
Make sure you're running from the project root or adjust the path:
```nu
nu /full/path/to/scripts/dev.nu watch
```

### Permission Denied
Make scripts executable:
```bash
chmod +x scripts/*.nu
```

### cargo-watch Not Found
The scripts will prompt to install it, or manually install:
```bash
cargo install cargo-watch
```
