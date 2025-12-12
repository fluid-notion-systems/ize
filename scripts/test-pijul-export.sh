#!/bin/bash

# Test script for pijul export functionality
# This script:
# 1. Creates a test directory with some files
# 2. Initializes ize tracking
# 3. Mounts the directory
# 4. Performs various file operations
# 5. Unmounts the directory
# 6. Exports to a temp directory
# 7. Runs pijul commands to inspect the result

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Output functions
success() { echo -e "${GREEN}✓${NC} $1"; }
info() { echo -e "${BLUE}→${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }
err() { echo -e "${RED}✗${NC} $1"; }
header() { echo -e "\n${BLUE}${BOLD}═══ $1 ═══${NC}\n"; }

# Parse arguments
KEEP=false
VERBOSE=false
while [[ $# -gt 0 ]]; do
    case $1 in
        -k|--keep)
            KEEP=true
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  -k, --keep     Keep test directories after completion"
            echo "  -v, --verbose  Verbose output"
            echo "  -h, --help     Show this help message"
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Get script directory and project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
IZE_BIN="$PROJECT_ROOT/target/debug/ize"

# Generate unique test directories
TEST_DIR="/tmp/ize-pijul-test-$$-$(date +%s)"
EXPORT_DIR="/tmp/ize-pijul-export-$$-$(date +%s)"

# Cleanup function
cleanup() {
    info "Cleaning up..."

    # Try to unmount if mounted
    fusermount -u "$TEST_DIR" 2>/dev/null || true

    # Remove test directory from ize tracking
    if [[ -x "$IZE_BIN" ]]; then
        "$IZE_BIN" remove "$TEST_DIR" --force 2>/dev/null || true
    fi

    # Remove directories
    rm -rf "$TEST_DIR" 2>/dev/null || true
    rm -rf "$EXPORT_DIR" 2>/dev/null || true
}

# Set up cleanup trap unless --keep is specified
if [[ "$KEEP" != "true" ]]; then
    trap cleanup EXIT
fi

header "Pijul Export Test"

# Check if ize binary exists, build if not
if [[ ! -x "$IZE_BIN" ]]; then
    err "ize binary not found at $IZE_BIN"
    info "Building ize..."
    cd "$PROJECT_ROOT"
    cargo build --package ize
    if [[ $? -ne 0 ]]; then
        err "Failed to build ize"
        exit 1
    fi
fi

success "Using ize binary: $IZE_BIN"

if [[ "$KEEP" == "true" ]]; then
    warn "Test directory: $TEST_DIR"
    warn "Export directory: $EXPORT_DIR"
    warn "(Keeping directories after test)"
else
    info "Test directory: $TEST_DIR"
    info "Export directory: $EXPORT_DIR"
    info "(Will be cleaned up after test)"
fi

# Step 1: Create test directory with initial files
header "Step 1: Creating test directory"
mkdir -p "$TEST_DIR"

echo "Hello, World!" > "$TEST_DIR/hello.txt"
printf "Line 1\nLine 2\nLine 3\n" > "$TEST_DIR/multiline.txt"
mkdir -p "$TEST_DIR/subdir"
echo "Nested file content" > "$TEST_DIR/subdir/nested.txt"

success "Created test directory with initial files"
if [[ "$VERBOSE" == "true" ]]; then
    echo "Files:"
    ls -la "$TEST_DIR"
fi

# Step 2: Initialize ize tracking
header "Step 2: Initializing ize tracking"
"$IZE_BIN" init "$TEST_DIR"
success "Initialized ize tracking"

# Step 3: Mount the directory
header "Step 3: Mounting directory"

# Run mount in background
"$IZE_BIN" mount "$TEST_DIR" --foreground &
MOUNT_PID=$!
sleep 2  # Give it time to mount

# Verify it's mounted
if ! grep -q "$TEST_DIR" /proc/mounts; then
    err "Directory doesn't appear to be mounted"
    kill $MOUNT_PID 2>/dev/null || true
    exit 1
fi
success "Directory mounted (PID: $MOUNT_PID)"

# Step 4: Perform file operations
header "Step 4: Performing file operations"

info "Writing to existing file..."
echo "Hello, Pijul!" > "$TEST_DIR/hello.txt"
success "Modified hello.txt"

info "Creating new file..."
echo "This is a new file created while mounted" > "$TEST_DIR/new_file.txt"
success "Created new_file.txt"

info "Appending to multiline file..."
printf "Line 4\nLine 5\n" >> "$TEST_DIR/multiline.txt"
success "Appended to multiline.txt"

info "Creating file in subdirectory..."
echo "Another nested file" > "$TEST_DIR/subdir/another.txt"
success "Created subdir/another.txt"

info "Creating new subdirectory with file..."
mkdir -p "$TEST_DIR/newdir"
echo "File in new directory" > "$TEST_DIR/newdir/file.txt"
success "Created newdir/file.txt"

# Give the filesystem time to process
sleep 1

if [[ "$VERBOSE" == "true" ]]; then
    echo ""
    echo "Current files:"
    ls -laR "$TEST_DIR"
fi

# Step 5: Unmount
header "Step 5: Unmounting directory"
"$IZE_BIN" unmount "$TEST_DIR" || {
    warn "Normal unmount failed, trying fusermount..."
    fusermount -u "$TEST_DIR"
}
sleep 1
success "Directory unmounted"

# Step 6: Export pijul repository
header "Step 6: Exporting pijul repository"
"$IZE_BIN" export-pijul "$TEST_DIR" "$EXPORT_DIR"
success "Exported to $EXPORT_DIR"

# Step 7: Inspect with pijul
header "Step 7: Inspecting with pijul"
cd "$EXPORT_DIR"

info "Directory structure:"
ls -la
echo ""

info "Running 'pijul list'..."
echo "---"
pijul list 2>/dev/null || warn "pijul list failed (may be empty repo)"
echo "---"
echo ""

info "Running 'pijul log'..."
echo "---"
pijul log 2>/dev/null || warn "pijul log failed (may be empty repo)"
echo "---"
echo ""

info "Running 'pijul diff'..."
echo "---"
pijul diff 2>/dev/null || warn "pijul diff failed"
echo "---"
echo ""

info "Checking file contents:"
if [[ -f "$EXPORT_DIR/hello.txt" ]]; then
    echo "hello.txt: $(cat "$EXPORT_DIR/hello.txt")"
fi
if [[ -f "$EXPORT_DIR/new_file.txt" ]]; then
    echo "new_file.txt: $(cat "$EXPORT_DIR/new_file.txt")"
fi
if [[ -f "$EXPORT_DIR/multiline.txt" ]]; then
    echo "multiline.txt:"
    cat "$EXPORT_DIR/multiline.txt"
fi

header "Test Complete"
success "Pijul export test finished successfully!"
echo ""
echo "You can inspect the exported repository at:"
echo "  cd $EXPORT_DIR"
echo ""
echo "Useful pijul commands:"
echo "  pijul log           # View change history"
echo "  pijul list          # List tracked files"
echo "  pijul diff          # Show uncommitted changes"
echo "  pijul channel list  # List channels"

if [[ "$KEEP" == "true" ]]; then
    echo ""
    warn "Test directories kept (use -k to disable):"
    warn "  Test dir:   $TEST_DIR"
    warn "  Export dir: $EXPORT_DIR"
    # Disable cleanup trap
    trap - EXIT
fi
