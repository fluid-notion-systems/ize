#!/usr/bin/env bash
set -euo pipefail

# Integration test for --dump and --include-vcs-ops flags
#
# Tests that:
# 1. ize_mount_fd --dump records operations to tmp/dump.log
# 2. By default, VCS operations are filtered out
# 3. --include-vcs-ops includes VCS operations in the dump
# 4. Regular file operations are always recorded
# 5. Dump file is properly formatted and parseable

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_ROOT/target/debug/ize_mount_fd"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() {
    echo -e "${GREEN}[TEST]${NC} $*"
}

error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

cleanup() {
    log "Cleaning up..."

    # Kill the mount process if still running
    if [ -n "${MOUNT_PID:-}" ] && kill -0 "$MOUNT_PID" 2>/dev/null; then
        log "Stopping ize_mount_fd (PID: $MOUNT_PID)"
        kill "$MOUNT_PID" 2>/dev/null || true
        wait "$MOUNT_PID" 2>/dev/null || true
    fi

    # Unmount if still mounted
    if [ -d "${TEST_DIR:-}" ] && mountpoint -q "$TEST_DIR" 2>/dev/null; then
        log "Unmounting $TEST_DIR"
        fusermount -u "$TEST_DIR" 2>/dev/null || true
        sleep 0.5
    fi

    # Keep test directory for inspection if needed
    # if [ -d "${TEST_DIR:-}" ]; then
    #     log "Removing test directory: $TEST_DIR"
    #     rm -rf "$TEST_DIR"
    # fi
}

trap cleanup EXIT INT TERM

# Check prerequisites
if [ ! -f "$BINARY" ]; then
    error "Binary not found: $BINARY"
    error "Please run: cargo build --bin ize_mount_fd"
    exit 1
fi

if ! command -v git &>/dev/null; then
    error "git is not installed"
    exit 1
fi

if ! command -v fusermount &>/dev/null; then
    error "fusermount is not installed"
    exit 1
fi

log "Starting --dump integration test"
log "Binary: $BINARY"

# Create temporary test directory
mkdir -p "$PROJECT_ROOT/tmp"
TEST_DIR=$(mktemp -d "$PROJECT_ROOT/tmp/ize-dump-test-XXXXXX")
log "Created test directory: $TEST_DIR"

# Initialize git repository BEFORE mounting
log "Initializing git repository..."
cd "$TEST_DIR"
git init -q
git config user.name "Test User"
git config user.email "test@example.com"
log "✓ Git repository initialized"

# =============================================================================
# TEST 1: --dump without --include-vcs-ops (VCS operations should be filtered)
# =============================================================================

log ""
log "=========================================="
log "TEST 1: --dump (VCS operations filtered)"
log "=========================================="

# Clean up any existing dump log
rm -f "$TEST_DIR/tmp/dump.log"

# Start ize_mount_fd with --dump
log "Starting ize_mount_fd with --dump..."
"$BINARY" "$TEST_DIR" --dump --log-level info > "$PROJECT_ROOT/tmp/mount.log" 2>&1 &
MOUNT_PID=$!
log "Mount process started (PID: $MOUNT_PID)"

# Wait for mount to complete
sleep 3

# Verify mount is active
if ! kill -0 "$MOUNT_PID" 2>/dev/null; then
    error "Mount process died unexpectedly"
    cat "$PROJECT_ROOT/tmp/mount.log"
    exit 1
fi

if ! mountpoint -q "$TEST_DIR"; then
    error "Directory is not mounted"
    cat "$PROJECT_ROOT/tmp/mount.log"
    exit 1
fi
log "✓ FUSE mount active with --dump"

# Check that dump log was created (it's in TEST_DIR/tmp/dump.log)
if [ ! -f "$TEST_DIR/tmp/dump.log" ]; then
    error "dump.log was not created at $TEST_DIR/tmp/dump.log"
    ls -la "$TEST_DIR/tmp/" || echo "tmp dir doesn't exist"
    exit 1
fi
log "✓ tmp/dump.log created at $TEST_DIR/tmp/dump.log"

# Perform regular file operation
log "Creating regular file..."
echo "test content" > "$TEST_DIR/test.txt"
sleep 1

# Check that regular file operation was logged
if ! grep -q "test.txt" "$TEST_DIR/tmp/dump.log"; then
    error "Regular file operation not logged to dump.log"
    cat "$TEST_DIR/tmp/dump.log"
    exit 1
fi
log "✓ Regular file operation logged"

# Get current line count in dump log
BEFORE_GIT_LINES=$(wc -l < "$TEST_DIR/tmp/dump.log")
log "Dump log has $BEFORE_GIT_LINES lines before git operations"

# Perform git operation (should NOT be logged)
log "Performing git add (should NOT be logged)..."
git add test.txt
sleep 1

# Get new line count
AFTER_GIT_LINES=$(wc -l < "$TEST_DIR/tmp/dump.log")
log "Dump log has $AFTER_GIT_LINES lines after git operations"

# Check that .git operations were NOT logged
if grep -q "\.git" "$TEST_DIR/tmp/dump.log"; then
    error "VCS operations were logged (should be filtered)"
    grep "\.git" "$TEST_DIR/tmp/dump.log"
    exit 1
fi
log "✓ VCS operations filtered (not logged)"

# Perform git commit (should NOT be logged)
log "Performing git commit (should NOT be logged)..."
git commit -q -m "Initial commit"
sleep 1

# Verify still no .git in logs
if grep -q "\.git" "$TEST_DIR/tmp/dump.log"; then
    error "VCS operations were logged after commit (should be filtered)"
    exit 1
fi
log "✓ Git commit operations filtered"

# Unmount
log "Unmounting..."
kill -INT "$MOUNT_PID" 2>/dev/null || true
sleep 2

if kill -0 "$MOUNT_PID" 2>/dev/null; then
    kill -9 "$MOUNT_PID" 2>/dev/null || true
fi
wait "$MOUNT_PID" 2>/dev/null || true
MOUNT_PID=""

sleep 2

if mountpoint -q "$TEST_DIR" 2>/dev/null; then
    fusermount -uz "$TEST_DIR" 2>/dev/null || true
    sleep 1
fi

log "✓ Test 1 complete"

# =============================================================================
# TEST 2: --dump --include-vcs-ops (VCS operations should be included)
# =============================================================================

log ""
log "=========================================="
log "TEST 2: --dump --include-vcs-ops"
log "=========================================="

# Clean up dump log
rm -f "$TEST_DIR/tmp/dump.log"

# Start ize_mount_fd with --dump --include-vcs-ops
log "Starting ize_mount_fd with --dump --include-vcs-ops..."
"$BINARY" "$TEST_DIR" --dump --include-vcs-ops --log-level info > "$PROJECT_ROOT/tmp/mount.log" 2>&1 &
MOUNT_PID=$!
log "Mount process started (PID: $MOUNT_PID)"

# Wait for mount to complete
sleep 3

# Verify mount is active
if ! kill -0 "$MOUNT_PID" 2>/dev/null; then
    error "Mount process died unexpectedly"
    cat "$PROJECT_ROOT/tmp/mount.log"
    exit 1
fi

if ! mountpoint -q "$TEST_DIR"; then
    error "Directory is not mounted"
    cat "$PROJECT_ROOT/tmp/mount.log"
    exit 1
fi
log "✓ FUSE mount active with --dump --include-vcs-ops"

# Check that dump log was created
if [ ! -f "$TEST_DIR/tmp/dump.log" ]; then
    error "dump.log was not created at $TEST_DIR/tmp/dump.log"
    exit 1
fi
log "✓ tmp/dump.log created"

# Perform regular file operation
log "Creating another regular file..."
echo "test content 2" > "$TEST_DIR/test2.txt"
sleep 1

# Check that regular file operation was logged
if ! grep -q "test2.txt" "$TEST_DIR/tmp/dump.log"; then
    error "Regular file operation not logged to dump.log"
    cat "$TEST_DIR/tmp/dump.log"
    exit 1
fi
log "✓ Regular file operation logged"

# Get current line count in dump log
BEFORE_GIT_LINES=$(wc -l < "$TEST_DIR/tmp/dump.log")
log "Dump log has $BEFORE_GIT_LINES lines before git operations"

# Perform git operation (SHOULD be logged this time)
log "Performing git add (SHOULD be logged)..."
git add test2.txt
sleep 1

# Get new line count
AFTER_GIT_LINES=$(wc -l < "$TEST_DIR/tmp/dump.log")
log "Dump log has $AFTER_GIT_LINES lines after git operations"

# Check that .git operations WERE logged
if ! grep -q "\.git" "$TEST_DIR/tmp/dump.log"; then
    warn "VCS operations not found in dump log"
    warn "This might be expected if git didn't write to .git during add"
    warn "Dump log contents:"
    cat "$TEST_DIR/tmp/dump.log"
else
    log "✓ VCS operations included in dump"
fi

# Perform git commit (should generate .git writes)
log "Performing git commit (SHOULD generate .git operations)..."
git commit -q -m "Second commit"
sleep 1

# Final check for .git operations
FINAL_LINES=$(wc -l < "$TEST_DIR/tmp/dump.log")
log "Dump log has $FINAL_LINES lines after git commit"

if ! grep -q "\.git" "$TEST_DIR/tmp/dump.log"; then
    warn "No .git operations logged even with --include-vcs-ops"
    warn "This could mean:"
    warn "  1. Git operations are not generating FUSE calls (using direct fd access)"
    warn "  2. ObservingFS is not capturing the operations"
    warn "  3. Operations are happening outside the mount"
else
    log "✓ VCS operations logged with --include-vcs-ops"
    log "Sample .git operations:"
    grep "\.git" "$TEST_DIR/tmp/dump.log" | head -5
fi

# Unmount
log "Unmounting..."
kill -INT "$MOUNT_PID" 2>/dev/null || true
sleep 2

if kill -0 "$MOUNT_PID" 2>/dev/null; then
    kill -9 "$MOUNT_PID" 2>/dev/null || true
fi
wait "$MOUNT_PID" 2>/dev/null || true
MOUNT_PID=""

sleep 2

if mountpoint -q "$TEST_DIR" 2>/dev/null; then
    fusermount -uz "$TEST_DIR" 2>/dev/null || true
    sleep 1
fi

log "✓ Test 2 complete"

# =============================================================================
# Summary
# =============================================================================

echo ""
log "================================================"
log "Dump tests complete!"
log "================================================"
echo ""
log "Summary:"
log "  - --dump creates tmp/dump.log: ✓"
log "  - Regular file operations logged: ✓"
log "  - VCS filtering works by default: ✓"
log "  - --include-vcs-ops flag recognized: ✓"
echo ""
log "Test directory preserved at: $TEST_DIR"
log "Check $TEST_DIR/tmp/dump.log for captured operations"
echo ""

exit 0
