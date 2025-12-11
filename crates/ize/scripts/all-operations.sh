#!/bin/bash
# Run through various filesystem operations to test opcode capture
#
# Run this script in a separate terminal while run-dump-opcode.sh is running.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MOUNT_DIR="$CRATE_DIR/tmp/mount"

echo "=== Ize Filesystem Operations Test ==="
echo ""
echo "Mount dir: $MOUNT_DIR"
echo ""

# Check if mount is accessible
if [ ! -d "$MOUNT_DIR" ]; then
    echo "ERROR: Mount directory does not exist: $MOUNT_DIR"
    echo "Make sure run-dump-opcode.sh is running first."
    exit 1
fi

# Small delay between operations so output is readable
DELAY=0.5

pause() {
    sleep $DELAY
}

echo "--- File Operations ---"
echo ""

echo "1. Creating a file..."
echo "Hello, Ize!" > "$MOUNT_DIR/hello.txt"
pause

echo "2. Reading file (should not generate opcode)..."
cat "$MOUNT_DIR/hello.txt"
pause

echo "3. Appending to file..."
echo "This is appended." >> "$MOUNT_DIR/hello.txt"
pause

echo "4. Overwriting file..."
echo "Completely new content" > "$MOUNT_DIR/hello.txt"
pause

echo "5. Creating another file..."
echo "Second file" > "$MOUNT_DIR/second.txt"
pause

echo "6. Renaming file..."
mv "$MOUNT_DIR/second.txt" "$MOUNT_DIR/renamed.txt"
pause

echo "7. Truncating file..."
truncate -s 5 "$MOUNT_DIR/hello.txt"
pause

echo "8. Changing permissions..."
chmod 600 "$MOUNT_DIR/hello.txt"
pause

echo "9. Deleting files..."
rm "$MOUNT_DIR/hello.txt"
rm "$MOUNT_DIR/renamed.txt"
pause

echo ""
echo "--- Directory Operations ---"
echo ""

echo "10. Creating directory..."
mkdir "$MOUNT_DIR/testdir"
pause

echo "11. Creating nested directories..."
mkdir -p "$MOUNT_DIR/parent/child/grandchild"
pause

echo "12. Creating file in subdirectory..."
echo "nested content" > "$MOUNT_DIR/testdir/nested.txt"
pause

echo "13. Renaming directory..."
mv "$MOUNT_DIR/testdir" "$MOUNT_DIR/renameddir"
pause

echo "14. Removing file from directory..."
rm "$MOUNT_DIR/renameddir/nested.txt"
pause

echo "15. Removing directories..."
rmdir "$MOUNT_DIR/renameddir"
rmdir "$MOUNT_DIR/parent/child/grandchild"
rmdir "$MOUNT_DIR/parent/child"
rmdir "$MOUNT_DIR/parent"
pause

echo ""
echo "--- Large File Operations ---"
echo ""

echo "16. Creating larger file (1KB)..."
dd if=/dev/urandom of="$MOUNT_DIR/random.bin" bs=1024 count=1 2>/dev/null
pause

echo "17. Partial write (overwrite middle)..."
echo "OVERWRITTEN" | dd of="$MOUNT_DIR/random.bin" bs=1 seek=100 conv=notrunc 2>/dev/null
pause

echo "18. Cleaning up large file..."
rm "$MOUNT_DIR/random.bin"
pause

echo ""
echo "--- Symlink Operations ---"
echo ""

echo "19. Creating target file..."
echo "target content" > "$MOUNT_DIR/target.txt"
pause

echo "20. Creating symlink..."
ln -s target.txt "$MOUNT_DIR/link.txt"
pause

echo "21. Reading through symlink (should not generate opcode)..."
cat "$MOUNT_DIR/link.txt"
pause

echo "22. Removing symlink..."
rm "$MOUNT_DIR/link.txt"
pause

echo "23. Removing target..."
rm "$MOUNT_DIR/target.txt"
pause

echo ""
echo "=== All operations complete! ==="
echo ""
echo "Check the opcode dump output in the other terminal."
