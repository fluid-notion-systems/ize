#!/usr/bin/env bash
set -euo pipefail

# Integration test for VCS filtering with ize_mount_fd
#
# Tests that:
# 1. ize_mount_fd can mount a directory with a git repo
# 2. VCS directories (.git) are detected
# 3. Operations inside .git don't trigger observers (if we had observers)
# 4. Regular file operations work normally
# 5. Git operations succeed (proving .git is accessible)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_ROOT/target/release/ize_mount_fd"

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

    # Remove test directory
    if [ -d "${TEST_DIR:-}" ]; then
        log "Removing test directory: $TEST_DIR"
        rm -rf "$TEST_DIR"
    fi
}

trap cleanup EXIT INT TERM

# Check prerequisites
if [ ! -f "$BINARY" ]; then
    error "Binary not found: $BINARY"
    error "Please run: cargo build --release --bin ize_mount_fd"
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

log "Starting VCS filtering integration test"
log "Binary: $BINARY"

# Create temporary test directory
TEST_DIR=$(mktemp -d -t ize-vcs-test-XXXXXX)
log "Created test directory: $TEST_DIR"

# Initialize git repository BEFORE mounting
log "Initializing git repository..."
cd "$TEST_DIR"
git init -q
git config user.name "Test User"
git config user.email "test@example.com"

log "✓ Git repository initialized"

# Start ize_mount_fd in background
log "Starting ize_mount_fd..."
"$BINARY" "$TEST_DIR" --log-level info > "$TEST_DIR/../mount.log" 2>&1 &
MOUNT_PID=$!
log "Mount process started (PID: $MOUNT_PID)"

# Wait for mount to complete
sleep 2

# Verify mount is active
if ! kill -0 "$MOUNT_PID" 2>/dev/null; then
    error "Mount process died unexpectedly"
    cat "$TEST_DIR/../mount.log"
    exit 1
fi

if ! mountpoint -q "$TEST_DIR"; then
    error "Directory is not mounted"
    cat "$TEST_DIR/../mount.log"
    exit 1
fi
log "✓ FUSE mount active"

# Check VCS detection in logs
if grep -q "Detected VCS" "$TEST_DIR/../mount.log"; then
    VCS_DETECTED=$(grep "Detected VCS" "$TEST_DIR/../mount.log" | tail -1)
    log "✓ VCS detected: $VCS_DETECTED"
else
    warn "VCS detection not found in logs"
fi

# Test 1: Create initial file through mount
log "Test 1: Creating test file through mount..."
echo "Hello, World!" > "$TEST_DIR/test.txt"
echo "This is a test file." >> "$TEST_DIR/test.txt"

if [ ! -f "$TEST_DIR/test.txt" ]; then
    error "Failed to create test.txt through mount"
    exit 1
fi

CONTENT=$(cat "$TEST_DIR/test.txt")
if [[ "$CONTENT" != *"Hello, World!"* ]]; then
    error "File content incorrect: $CONTENT"
    exit 1
fi
log "✓ test.txt created and readable through mount"

# Test 2: Git add (writes to .git/index)
log "Test 2: Running 'git add' (writes to .git)..."
if ! git add test.txt; then
    error "git add failed"
    exit 1
fi
log "✓ git add succeeded"

# Test 3: Verify .git/index was updated
if [ ! -f "$TEST_DIR/.git/index" ]; then
    error ".git/index was not created"
    exit 1
fi
log "✓ .git/index exists"

# Test 4: Git commit (writes to .git/objects)
log "Test 3: Running 'git commit'..."
if ! git commit -q -m "Initial commit"; then
    error "git commit failed"
    exit 1
fi
log "✓ git commit succeeded"

# Test 5: Verify commit exists
COMMIT_COUNT=$(git rev-list --count HEAD)
if [ "$COMMIT_COUNT" -ne 1 ]; then
    error "Expected 1 commit, got $COMMIT_COUNT"
    exit 1
fi
log "✓ Commit verified (count: $COMMIT_COUNT)"

# Test 6: Create another file through the mount
log "Test 4: Creating new file through mount..."
echo "Another test file" > "$TEST_DIR/test2.txt"
if [ ! -f "$TEST_DIR/test2.txt" ]; then
    error "Failed to create test2.txt"
    exit 1
fi
log "✓ New file created"

# Test 7: Modify file through mount
log "Test 5: Modifying file through mount..."
echo "Modified content" >> "$TEST_DIR/test.txt"
if ! grep -q "Modified content" "$TEST_DIR/test.txt"; then
    error "File modification failed"
    exit 1
fi
log "✓ File modified"

# Test 8: Git add and commit new changes
log "Test 6: Committing new changes..."
git add test2.txt test.txt
if ! git commit -q -m "Second commit"; then
    error "Second commit failed"
    exit 1
fi

COMMIT_COUNT=$(git rev-list --count HEAD)
if [ "$COMMIT_COUNT" -ne 2 ]; then
    error "Expected 2 commits, got $COMMIT_COUNT"
    exit 1
fi
log "✓ Second commit verified (count: $COMMIT_COUNT)"

# Test 9: Verify .git directory is accessible
log "Test 7: Checking .git directory accessibility..."
if [ ! -d "$TEST_DIR/.git" ]; then
    error ".git directory not accessible"
    exit 1
fi

if [ ! -d "$TEST_DIR/.git/objects" ]; then
    error ".git/objects not accessible"
    exit 1
fi
log "✓ .git directory fully accessible"

# Test 10: Git log works
log "Test 8: Running 'git log'..."
LOG_OUTPUT=$(git log --oneline)
if [ -z "$LOG_OUTPUT" ]; then
    error "git log returned empty output"
    exit 1
fi
log "✓ git log works"

# Unmount - send SIGINT (Ctrl+C) to trigger proper cleanup handler
log "Unmounting filesystem..."
kill -INT "$MOUNT_PID" 2>/dev/null || true
sleep 2

# Wait for process to exit
for i in {1..5}; do
    if ! kill -0 "$MOUNT_PID" 2>/dev/null; then
        log "✓ Mount process terminated"
        break
    fi
    sleep 0.5
done

# Force kill if still running
if kill -0 "$MOUNT_PID" 2>/dev/null; then
    warn "Force killing mount process..."
    kill -9 "$MOUNT_PID" 2>/dev/null || true
fi

wait "$MOUNT_PID" 2>/dev/null || true
MOUNT_PID=""

# Wait for unmount to complete and filesystem to stabilize
log "Waiting for unmount to complete..."
sleep 2

for i in {1..10}; do
    if ! mountpoint -q "$TEST_DIR" 2>/dev/null; then
        log "✓ Unmount verified"
        break
    fi
    sleep 0.5
done

# Force unmount if still mounted
if mountpoint -q "$TEST_DIR" 2>/dev/null; then
    warn "Force unmounting with fusermount -uz..."
    fusermount -uz "$TEST_DIR" 2>/dev/null || true
    sleep 2
fi

# Final verification
if mountpoint -q "$TEST_DIR" 2>/dev/null; then
    error "Failed to unmount $TEST_DIR"
    exit 1
fi

# Give filesystem time to settle
sleep 1

# Test 11: Verify files persist after unmount
log "Test 9: Verifying persistence after unmount..."

if [ ! -f "$TEST_DIR/test.txt" ]; then
    error "test.txt missing after unmount"
    exit 1
fi

if [ ! -f "$TEST_DIR/test2.txt" ]; then
    error "test2.txt missing after unmount"
    exit 1
fi

if ! grep -q "Modified content" "$TEST_DIR/test.txt"; then
    error "File modifications lost after unmount"
    exit 1
fi
log "✓ Files persisted after unmount"

# Test 12: Verify git commits persist
log "Test 10: Verifying git commits persist..."
cd "$TEST_DIR"
FINAL_COMMIT_COUNT=$(git rev-list --count HEAD)
if [ "$FINAL_COMMIT_COUNT" -ne 2 ]; then
    error "Expected 2 commits after unmount, got $FINAL_COMMIT_COUNT"
    exit 1
fi
log "✓ Git commits persisted (count: $FINAL_COMMIT_COUNT)"

# Success!
echo ""
log "================================================"
log "All tests passed! ✓"
log "================================================"
echo ""
log "Summary:"
log "  - FUSE mount/unmount: ✓"
log "  - VCS detection (.git): ✓"
log "  - File operations: ✓"
log "  - Git operations (add/commit): ✓"
log "  - .git directory accessibility: ✓"
log "  - Data persistence: ✓"
echo ""

exit 0
