#!/usr/bin/env nu

# Simple watch script for Claris-FUSE
# Run from project root: nu scripts/w.nu

# Just cd to the right directory and run cargo watch
cd crates/claris-fuse-lib

print "Starting watch mode..."
print "• Test files → cargo test"
print "• Source files → cargo build"
print ""

# Run cargo watch with simple shell command
cargo watch --clear -x build -s "find tests -newer .build-marker 2>/dev/null | grep -q '.rs$' && cargo test || true; touch .build-marker"
