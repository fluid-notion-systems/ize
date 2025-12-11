#!/bin/bash
# Run the opcode dump tool
#
# This script builds the tool, sets up temp directories, and runs the dump tool.
# In another terminal, run ./scripts/all-operations.sh to perform test operations.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

# Directories
SRC_DIR="$CRATE_DIR/tmp/src"
MOUNT_DIR="$CRATE_DIR/tmp/mount"

echo "=== Ize Opcode Dump Tool ==="
echo ""
echo "Project root: $PROJECT_ROOT"
echo "Source dir:   $SRC_DIR"
echo "Mount dir:    $MOUNT_DIR"
echo ""

# Build the tool
echo "Building ize_dump_opcode_queue..."
cd "$PROJECT_ROOT"
cargo build --package ize --bin ize_dump_opcode_queue

# Create temp directories
echo "Creating temp directories..."
mkdir -p "$SRC_DIR" "$MOUNT_DIR"

# Clean up on exit
cleanup() {
    echo ""
    echo "Cleaning up..."
    fusermount -u "$MOUNT_DIR" 2>/dev/null || true
}
trap cleanup EXIT

echo ""
echo "Starting opcode dump tool..."
echo "In another terminal, run:"
echo "  cd $CRATE_DIR && ./scripts/all-operations.sh"
echo ""

# Run the tool
"$PROJECT_ROOT/target/debug/ize_dump_opcode_queue" "$SRC_DIR" "$MOUNT_DIR"
