#!/bin/bash

# Claris-FUSE Development Watch Script
# Automatically runs tests when test files change, builds when source files change

set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if cargo-watch is installed
if ! command -v cargo-watch &> /dev/null; then
    echo -e "${YELLOW}cargo-watch is not installed.${NC}"
    echo "Installing cargo-watch..."
    cargo install cargo-watch
fi

echo -e "${BLUE}Starting Claris-FUSE watch mode...${NC}"
echo -e "${GREEN}• Test files (tests/**/*.rs) → cargo test${NC}"
echo -e "${GREEN}• Source files (src/**/*.rs) → cargo build${NC}"
echo ""

# Function to determine what to run based on changed files
run_appropriate_command() {
    # Get list of changed files from cargo-watch (passed as arguments)
    local changed_files="$@"

    # Check if any test files were changed
    if echo "$changed_files" | grep -q "tests/"; then
        echo -e "${BLUE}Test files changed, running tests...${NC}"
        cargo test
    else
        echo -e "${BLUE}Source files changed, building...${NC}"
        cargo build
    fi
}

# Use cargo-watch with a custom shell command
# The -s flag allows us to run a shell command
# The --why flag shows which files triggered the rebuild
cd crates/claris-fuse-lib

cargo watch \
    --clear \
    --why \
    --watch src \
    --watch tests \
    --watch Cargo.toml \
    --shell 'bash -c "
        # Cargo watch sets $CARGO_WATCH_REASON with the changed files
        if [[ \"$CARGO_WATCH_REASON\" == *\"tests/\"* ]]; then
            echo -e \"\033[0;34mTest files changed, running tests...\033[0m\"
            cargo test --color=always
        elif [[ \"$CARGO_WATCH_REASON\" == *\"Cargo.toml\"* ]]; then
            echo -e \"\033[0;34mCargo.toml changed, running build and tests...\033[0m\"
            cargo build --color=always && cargo test --color=always
        else
            echo -e \"\033[0;34mSource files changed, building...\033[0m\"
            cargo build --color=always
        fi
    "'
