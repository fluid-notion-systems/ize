#!/usr/bin/env bash
# Repro: git commit hangs under ize_mount_fd FUSE mount (ize-wg8)
#
# Usage: ./scripts/bugs/git-commit-hang.sh [--dump]
# Kill with Ctrl+C or: fusermount -u /tmp/ize-bug-git-commit

set -euo pipefail

DIR="/tmp/ize-bug-git-commit"
MOUNT_PID=""
BINARY="./target/debug/ize_mount_fd"

cleanup() {
    echo ""
    echo "--- cleanup ---"
    fusermount -u "$DIR" 2>/dev/null || true
    [[ -n "$MOUNT_PID" ]] && kill "$MOUNT_PID" 2>/dev/null || true
    wait "$MOUNT_PID" 2>/dev/null || true
    echo "done"
}
trap cleanup EXIT

# Build
cargo build --bin ize_mount_fd 2>&1 | tail -1

# Fresh dir with a git repo
rm -rf "$DIR"
mkdir -p "$DIR"
git init "$DIR" --quiet
git -C "$DIR" config user.email "test@test.com"
git -C "$DIR" config user.name "Test"

# Seed a file so there's something to commit
echo "hello" > "$DIR/file.txt"
git -C "$DIR" add file.txt

# Mount
EXTRA_FLAGS=""
[[ "${1:-}" == "--dump" ]] && EXTRA_FLAGS="--dump"
echo "mounting $DIR ..."
$BINARY "$DIR" --log-level debug $EXTRA_FLAGS &
MOUNT_PID=$!
sleep 1

echo "--- git status ---"
timeout 5 git -C "$DIR" status || echo "!! git status timed out"

echo "--- git commit ---"
timeout 10 git -C "$DIR" commit -m "test commit" 2>&1 || echo "!! git commit timed out / failed (exit $?)"

echo "--- done ---"
