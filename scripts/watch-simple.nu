#!/usr/bin/env nu

# Ultra-simple watch commands for Claris-FUSE
# Run from project root

# Watch and test
def wt [] {
    cd crates/claris-fuse-lib
    cargo watch -x test
}

# Watch and build
def wb [] {
    cd crates/claris-fuse-lib
    cargo watch -x build
}

# Watch and do both
def wa [] {
    cd crates/claris-fuse-lib
    cargo watch -x build -x test
}

# Default: just run tests
def main [] {
    wt
}

# Show help
def "main help" [] {
    print "Simple watch commands:"
    print "  nu scripts/watch-simple.nu      - Watch and run tests"
    print "  nu scripts/watch-simple.nu help - Show this help"
    print ""
    print "Or source it and use functions:"
    print "  source scripts/watch-simple.nu"
    print "  wt  - Watch and test"
    print "  wb  - Watch and build"
    print "  wa  - Watch all (build + test)"
}
